//! The host's side of a link: three bash scripts run over the existing SSH
//! layer (`SshExec::run_with_stdin`, so through the host's ControlMaster)
//! and the parsing of what they print. They need nothing a fleet host does
//! not already have: `bash`, `git`, `tar`, `stat`, `sha256sum` or `shasum`.
//! GNU and BSD userlands are both handled. A login profile's chatter on
//! stdout is skipped: every answer starts after a marker line.

use super::local::safe_rel;
use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshExec;
use crate::store::FileStat;
use std::collections::HashMap;
use std::time::Duration;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const SCAN_WALL_CLOCK: Duration = Duration::from_secs(120);
const TRANSFER_WALL_CLOCK: Duration = Duration::from_secs(600);
/// A scan's answer, at most (≈ 100 bytes a file).
const SCAN_MAX_OUTPUT: usize = 64 * 1024 * 1024;

const SCAN_MARK: &str = "@@FLEET-SCAN@@";
const TAR_MARK: &str = "@@FLEET-TAR@@\n";
const PUSH_MARK: &str = "@@FLEET-PUSH@@\n";
const ERR_MARK: &str = "@@FLEET-ERR@@";

/// `cd` into the root or say why not. Exit 3: the directory is missing.
pub(super) fn prologue(root: &str) -> String {
    format!(
        "set -u\ncd -- {} 2>/dev/null || {{ printf '{ERR_MARK} missing\\n'; exit 3; }}\n",
        quote(root)
    )
}

/// The scan. Stdin: NUL-separated paths to stat even when git no longer
/// lists them (the BASE). Prints the userland (`gnu` / `bsd`) and the host's
/// clock after the marker, then one record per regular file, link or
/// directory: `type \t size \t mtime \t path`, NUL-terminated on GNU,
/// newline-terminated on BSD (whose `stat` cannot print a NUL).
pub(super) fn scan_script(root: &str) -> String {
    format!(
        r#"{prologue}git rev-parse --git-dir >/dev/null 2>&1 || {{ printf '{ERR_MARK} notgit\n'; exit 4; }}
b=$(mktemp) || exit 5
trap 'rm -f -- "$b"' EXIT
cat > "$b"
if stat --version >/dev/null 2>&1; then m=gnu; else m=bsd; fi
printf '{SCAN_MARK} %s %s\n' "$m" "$(date +%s)"
{{ git ls-files -co --exclude-standard -z 2>/dev/null; cat -- "$b"; }} | LC_ALL=C sort -zu |
if [ "$m" = gnu ]; then
  xargs -0 stat --printf '%F\t%s\t%Y\t%n\0' -- 2>/dev/null
else
  xargs -0 stat -f '%HT%t%z%t%m%t%N' -- 2>/dev/null
fi
exit 0
"#,
        prologue = prologue(root)
    )
}

/// The download. Stdin: NUL-separated paths. Prints a tar of them after the
/// marker (a path that vanished meanwhile is simply not in it).
pub(super) fn pull_script(root: &str) -> String {
    format!(
        "{}printf '{}'\nCOPYFILE_DISABLE=1 tar -cf - --null -T - 2>/dev/null\nexit 0\n",
        prologue(root),
        TAR_MARK.trim_end().to_string() + "\\n"
    )
}

/// The guarded upload. Stdin: a tar holding `.m` — NUL-terminated records
/// `op \t expected \t path` (`W` write, `D` delete; `expected` is the sha256
/// the remote file must still have, `-` for "must be absent") — and the
/// files to write under `f/`. Extracts into a temp directory inside the
/// root (so the final `mv` is a rename on one filesystem), then for each
/// record compares, and only on a match moves the file into place or
/// deletes it. A path below a link or a file counts as a link (`L`), so
/// nothing is written or deleted outside the root through a symlinked
/// directory. Files land with their arrival time (`tar -m`), not the
/// desktop's mtime: a build that ran on the host since the edit must still
/// see the file as newer than its outputs. Answers `O \t size \t mtime \t
/// path` for done, `C \t \t \t path` for a file that was not what was
/// expected, `X \t \t \t path` for a failure, each NUL-terminated.
pub(super) fn push_script(root: &str) -> String {
    format!(
        r#"{prologue}t=$(mktemp -d ./.fleet-sync-XXXXXX) || {{ printf '{ERR_MARK} mktemp\n'; exit 5; }}
trap 'rm -rf -- "$t"' EXIT
tar -xmf - -C "$t" 2>/dev/null || {{ printf '{ERR_MARK} tar\n'; exit 5; }}
if command -v sha256sum >/dev/null 2>&1; then h() {{ sha256sum < "$1" | cut -c1-64; }}
else h() {{ shasum -a 256 < "$1" | cut -c1-64; }}; fi
if stat --version >/dev/null 2>&1; then st() {{ stat --printf '%s\t%Y' -- "$1"; }}
else st() {{ stat -f '%z%t%m' -- "$1"; }}; fi
pl() {{ d=$(dirname -- "$1"); while [ "$d" != . ]; do
  if [ -L "$d" ] || {{ [ -e "$d" ] && [ ! -d "$d" ]; }}; then return 0; fi; d=$(dirname -- "$d"); done; return 1; }}
printf '{push}'
while IFS= read -r -d '' rec; do
  op=${{rec%%$'\t'*}}; rec=${{rec#*$'\t'}}; exp=${{rec%%$'\t'*}}; p=${{rec#*$'\t'}}
  if pl "$p" || [ -L "$p" ]; then cur=L; elif [ -f "$p" ]; then cur=$(h "$p"); elif [ -e "$p" ]; then cur=D; else cur=-; fi
  if [ "$cur" != "$exp" ]; then printf 'C\t\t\t%s\0' "$p"; continue; fi
  if [ "$op" = W ]; then
    if mkdir -p -- "$(dirname -- "$p")" 2>/dev/null && mv -f -- "$t/f/$p" "$p" 2>/dev/null; then
      printf 'O\t%s\t%s\0' "$(st "$p")" "$p"
    else printf 'X\t\t\t%s\0' "$p"; fi
  else
    if rm -f -- "$p" 2>/dev/null; then
      rmdir -p -- "$(dirname -- "$p")" 2>/dev/null
      printf 'O\t\t\t%s\0' "$p"
    else printf 'X\t\t\t%s\0' "$p"; fi
  fi
done < "$t/.m"
exit 0
"#,
        prologue = prologue(root),
        push = PUSH_MARK.trim_end().to_string() + "\\n"
    )
}

/// What one remote path is.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Kind {
    File,
    Symlink,
    Other,
}

#[derive(Debug, Default)]
pub(super) struct RemoteScan {
    pub entries: HashMap<String, (Kind, FileStat)>,
    /// The host's clock when it scanned (seconds).
    pub now: i64,
}

/// Run one script with `stdin` and return what it printed after `marker`.
pub(super) async fn run(
    ssh: &dyn SshExec,
    host: &str,
    script: &str,
    stdin: Vec<u8>,
    marker: &str,
    wall_clock: Duration,
    max_output: usize,
) -> Result<Vec<u8>, IpcError> {
    let quoted = quote(script);
    let args = ["bash", "-lc", quoted.as_str()];
    let out = ssh
        .run_with_stdin(host, &args, stdin, CONNECT_TIMEOUT, wall_clock, max_output)
        .await?;
    if out.stdout.len() >= max_output {
        return Err(IpcError::new(
            codes::E_IO,
            format!(
                "{host}: the answer was larger than {} MiB",
                max_output >> 20
            ),
        ));
    }
    let code = out.status.code().unwrap_or(-1);
    if code == 255 {
        return Err(IpcError::new(
            codes::E_SSH,
            format!(
                "{host} did not answer: {}",
                String::from_utf8_lossy(&out.stderr).trim()
            ),
        ));
    }
    // The answer's own marker first: a file in a tar may well contain the
    // error marker's text (this very source file does).
    if let Some(at) = find(&out.stdout, marker.as_bytes()) {
        return Ok(out.stdout[at + marker.len()..].to_vec());
    }
    if let Some(at) = find(&out.stdout, ERR_MARK.as_bytes()) {
        let why = String::from_utf8_lossy(&out.stdout[at + ERR_MARK.len()..])
            .trim()
            .to_string();
        return Err(match why.as_str() {
            "missing" => IpcError::new(
                codes::E_NOTFOUND,
                format!("the worktree folder is missing on {host}"),
            ),
            "notgit" => IpcError::new(
                codes::E_NOREPO,
                format!("the worktree folder on {host} is not a git checkout"),
            ),
            other => IpcError::new(codes::E_IO, format!("{host}: {other}")),
        });
    }
    Err(IpcError::new(
        codes::E_IO,
        format!(
            "{host}: the sync script did not run (exit {code}): {}",
            String::from_utf8_lossy(&out.stderr).trim()
        ),
    ))
}

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

pub(super) fn nul_list<'a>(paths: impl IntoIterator<Item = &'a String>) -> Vec<u8> {
    let mut v = Vec::new();
    for p in paths {
        v.extend_from_slice(p.as_bytes());
        v.push(0);
    }
    v
}

pub(super) async fn scan(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    also: &[String],
) -> Result<RemoteScan, IpcError> {
    let body = run(
        ssh,
        host,
        &scan_script(root),
        nul_list(also),
        SCAN_MARK,
        SCAN_WALL_CLOCK,
        SCAN_MAX_OUTPUT,
    )
    .await?;
    parse_scan(&body)
}

/// Parse the scan's answer (everything after the marker).
pub(super) fn parse_scan(body: &[u8]) -> Result<RemoteScan, IpcError> {
    let bad = || IpcError::new(codes::E_IO, "the remote scan answered something unreadable");
    let nl = body.iter().position(|b| *b == b'\n').ok_or_else(bad)?;
    let head = String::from_utf8_lossy(&body[..nl]);
    let mut words = head.split_whitespace();
    let mode = words.next().ok_or_else(bad)?;
    let now: i64 = words.next().and_then(|n| n.parse().ok()).ok_or_else(bad)?;
    let sep = if mode == "gnu" { b'\0' } else { b'\n' };
    let mut out = RemoteScan {
        entries: HashMap::new(),
        now,
    };
    for rec in body[nl + 1..].split(|b| *b == sep) {
        let Ok(rec) = std::str::from_utf8(rec) else {
            continue;
        };
        let mut f = rec.splitn(4, '\t');
        let (Some(ty), Some(size), Some(mtime), Some(path)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let (Ok(size), Ok(mtime)) = (size.parse::<i64>(), mtime.parse::<i64>()) else {
            continue;
        };
        let path = path.strip_prefix("./").unwrap_or(path);
        if !safe_rel(path) {
            continue;
        }
        let ty = ty.to_ascii_lowercase();
        let kind = if ty.contains("regular") {
            Kind::File
        } else if ty.contains("symbolic") {
            Kind::Symlink
        } else {
            Kind::Other
        };
        out.entries
            .insert(path.to_string(), (kind, FileStat { size, mtime }));
    }
    Ok(out)
}

/// Download `paths`; answers each file's bytes and tar mode. A path that is
/// not in the answer vanished meanwhile.
pub(super) async fn pull(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    paths: &[String],
    expected_bytes: u64,
) -> Result<HashMap<String, (Vec<u8>, u32)>, IpcError> {
    let cap = expected_bytes as usize + paths.len() * 1536 + 4 * 1024 * 1024;
    let body = run(
        ssh,
        host,
        &pull_script(root),
        nul_list(paths),
        TAR_MARK,
        TRANSFER_WALL_CLOCK,
        cap,
    )
    .await?;
    let wanted: std::collections::HashSet<&str> = paths.iter().map(String::as_str).collect();
    let mut out = HashMap::new();
    let mut ar = tar::Archive::new(body.as_slice());
    let entries = ar
        .entries()
        .map_err(|e| IpcError::new(codes::E_IO, format!("{host}: unreadable tar: {e}")))?;
    for entry in entries {
        let mut entry = entry
            .map_err(|e| IpcError::new(codes::E_IO, format!("{host}: unreadable tar: {e}")))?;
        if entry.header().entry_type() != tar::EntryType::Regular {
            continue;
        }
        let Ok(path) = entry.path() else { continue };
        let Some(path) = path
            .to_str()
            .map(|p| p.strip_prefix("./").unwrap_or(p).to_string())
        else {
            continue;
        };
        if !wanted.contains(path.as_str()) {
            continue;
        }
        let mode = entry.header().mode().unwrap_or(0o644);
        let mut bytes = Vec::with_capacity(entry.size() as usize);
        std::io::Read::read_to_end(&mut entry, &mut bytes)
            .map_err(|e| IpcError::new(codes::E_IO, format!("{host}: unreadable tar: {e}")))?;
        out.insert(path, (bytes, mode));
    }
    Ok(out)
}

/// One record of a guarded upload.
pub(super) enum PushOp {
    Write {
        path: String,
        bytes: Vec<u8>,
        executable: bool,
        mtime_secs: u64,
        expect: Option<String>,
    },
    Delete {
        path: String,
        expect: String,
    },
}

impl PushOp {
    pub(super) fn path(&self) -> &str {
        match self {
            PushOp::Write { path, .. } | PushOp::Delete { path, .. } => path,
        }
    }
}

/// How one record of an upload ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum PushResult {
    /// Done; the remote stat now (`None` after a delete).
    Done(Option<FileStat>),
    /// The remote file was not what was expected: left alone.
    Moved,
    Failed,
}

/// Build the upload's tar: `.m` first, then the files under `f/`.
pub(super) fn push_tar(ops: &[PushOp]) -> Result<Vec<u8>, IpcError> {
    let tar_err = |e: std::io::Error| IpcError::new(codes::E_IO, format!("build tar: {e}"));
    let mut manifest = Vec::new();
    for op in ops {
        let (code, expect) = match op {
            PushOp::Write { expect, .. } => ("W", expect.as_deref().unwrap_or("-")),
            PushOp::Delete { expect, .. } => ("D", expect.as_str()),
        };
        manifest.extend_from_slice(format!("{code}\t{expect}\t{}", op.path()).as_bytes());
        manifest.push(0);
    }
    let mut b = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_gnu();
    h.set_size(manifest.len() as u64);
    h.set_mode(0o600);
    h.set_entry_type(tar::EntryType::Regular);
    b.append_data(&mut h, ".m", manifest.as_slice())
        .map_err(tar_err)?;
    for op in ops {
        if let PushOp::Write {
            path,
            bytes,
            executable,
            mtime_secs,
            ..
        } = op
        {
            let mut h = tar::Header::new_gnu();
            h.set_size(bytes.len() as u64);
            h.set_mode(if *executable { 0o755 } else { 0o644 });
            h.set_mtime(*mtime_secs);
            h.set_entry_type(tar::EntryType::Regular);
            b.append_data(&mut h, format!("f/{path}"), bytes.as_slice())
                .map_err(tar_err)?;
        }
    }
    b.into_inner().map_err(tar_err)
}

pub(super) async fn push(
    ssh: &dyn SshExec,
    host: &str,
    root: &str,
    ops: &[PushOp],
) -> Result<HashMap<String, PushResult>, IpcError> {
    let tar = push_tar(ops)?;
    let body = run(
        ssh,
        host,
        &push_script(root),
        tar,
        PUSH_MARK,
        TRANSFER_WALL_CLOCK,
        // One short record per file.
        ops.len() * 4200 + 64 * 1024,
    )
    .await?;
    Ok(parse_push(&body))
}

pub(super) fn parse_push(body: &[u8]) -> HashMap<String, PushResult> {
    let mut out = HashMap::new();
    for rec in body.split(|b| *b == 0) {
        let Ok(rec) = std::str::from_utf8(rec) else {
            continue;
        };
        let mut f = rec.splitn(4, '\t');
        let (Some(code), Some(size), Some(mtime), Some(path)) =
            (f.next(), f.next(), f.next(), f.next())
        else {
            continue;
        };
        let result = match code {
            "O" => match (size.parse::<i64>(), mtime.parse::<i64>()) {
                (Ok(size), Ok(mtime)) => PushResult::Done(Some(FileStat { size, mtime })),
                _ => PushResult::Done(None),
            },
            "C" => PushResult::Moved,
            _ => PushResult::Failed,
        };
        out.insert(path.to_string(), result);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gnu_and_bsd_scans_parse_to_the_same_entries() {
        let gnu = b"gnu 1700000000\nregular file\t3\t1699999990\tsrc/a.rs\0symbolic link\t5\t1\tln\0directory\t4096\t1\tsub\0regular empty file\t0\t2\t./e\0regular file\t1\t1\t../evil\0";
        let s = parse_scan(gnu).unwrap();
        assert_eq!(s.now, 1_700_000_000);
        assert_eq!(
            s.entries["src/a.rs"],
            (
                Kind::File,
                FileStat {
                    size: 3,
                    mtime: 1_699_999_990
                }
            )
        );
        assert_eq!(s.entries["ln"].0, Kind::Symlink);
        assert_eq!(s.entries["sub"].0, Kind::Other);
        assert_eq!(s.entries["e"].0, Kind::File);
        assert!(!s.entries.contains_key("../evil"));

        let bsd = b"bsd 5\nRegular File\t3\t1699999990\tsrc/a.rs\nSymbolic Link\t5\t1\tln\n";
        let s = parse_scan(bsd).unwrap();
        assert_eq!(s.entries["src/a.rs"].0, Kind::File);
        assert_eq!(s.entries["ln"].0, Kind::Symlink);
    }

    #[test]
    fn push_results_parse() {
        let body = b"O\t3\t17\ta.txt\0C\t\t\tb.txt\0X\t\t\tc.txt\0O\t\t\td.txt\0";
        let r = parse_push(body);
        assert_eq!(
            r["a.txt"],
            PushResult::Done(Some(FileStat { size: 3, mtime: 17 }))
        );
        assert_eq!(r["b.txt"], PushResult::Moved);
        assert_eq!(r["c.txt"], PushResult::Failed);
        assert_eq!(r["d.txt"], PushResult::Done(None));
    }
}
