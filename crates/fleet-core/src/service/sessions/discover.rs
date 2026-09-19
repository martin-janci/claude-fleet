//! Host-reboot recovery, discovery half. Task 7 (below) parses and ranks the
//! Claude transcripts a host actually has on disk, independent of anything
//! fleet's own DB knows about — pure/read-only, no ssh, no store, no writes.
//! Task 8 ([`discover_lost_sessions`]) wires that to a live host (running
//! [`crate::tmux::discover_transcripts_script`] over ssh/local exec) and
//! fills in `derived_tmux_name`/`project_id`/`worktree_id`/
//! `existing_session_id`, which Task 7's [`rank_candidates`] deliberately
//! leaves `None`. Also read-only: one short store lock AFTER the ssh call
//! enriches the candidates the script found — nothing is written.

use super::*;
#[cfg(test)]
use crate::ipc_error::codes;
use crate::ipc_error::lock;
use serde::Serialize;

/// One transcript found by [`crate::tmux::discover_transcripts_script`]:
/// its Claude session id (the `.jsonl` file's basename, already validated
/// via [`crate::validate::claude_session_id`]), its mtime, and whatever
/// `cwd`/`gitBranch` could be recovered from its last `"cwd"`-bearing line.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptProbe {
    pub claude_session_id: String,
    pub mtime: i64,
    pub cwd: Option<String>,
    pub git_branch: Option<String>,
}

/// Find the JSON string literal value of `"<key>":"<value>"` in `text`,
/// starting the search at `key` (which must include the trailing colon,
/// e.g. `"cwd":`). Skips whitespace after the key, requires the next
/// non-whitespace character to be `"`, then scans for the matching
/// unescaped closing quote (byte-wise: multi-byte UTF-8 continuation bytes
/// never collide with the ASCII `\` / `"` we scan for) and decodes that
/// slice with `serde_json::from_str::<String>` so backslash escapes resolve
/// correctly. Absent key, no opening quote, or no matching closing quote
/// (e.g. the transcript line was truncated mid-string by `cut -c1-8192`)
/// all yield `None`.
fn extract_json_string_field(text: &str, key: &str) -> Option<String> {
    let idx = text.find(key)?;
    let after = text[idx + key.len()..].trim_start();
    if !after.starts_with('"') {
        return None;
    }
    let bytes = after.as_bytes();
    let mut i = 1;
    let mut escaped = false;
    while i < bytes.len() {
        let c = bytes[i];
        if escaped {
            escaped = false;
        } else if c == b'\\' {
            escaped = true;
        } else if c == b'"' {
            return serde_json::from_str::<String>(&after[..=i]).ok();
        }
        i += 1;
    }
    None
}

/// Parse macOS's `sysctl -n kern.boottime` output, e.g.
/// `{ sec = 1726000000, usec = 0 } Thu Sep 11 00:00:00 2025`: the integer
/// run of digits right after `sec = `. Empty (sysctl failed, redirected to
/// `/dev/null` by the script) or malformed input yields `None`.
fn parse_bootraw(value: &str) -> Option<i64> {
    let after = value.split_once("sec = ")?.1;
    let digits: String = after.chars().take_while(|c| c.is_ascii_digit()).collect();
    if digits.is_empty() {
        return None;
    }
    digits.parse::<i64>().ok()
}

/// Parse [`crate::tmux::discover_transcripts_script`]'s stdout into a boot
/// epoch (`bootsec=`/`bootraw=` line — see [`parse_bootraw`] for the latter)
/// and the list of transcripts it found. Every transcript is announced by an
/// `@@F\t<mtime>\t<claude session id>` line, immediately followed by an
/// `@@L\t<line>` line carrying the transcript's last `"cwd"`-bearing line
/// (verbatim, possibly truncated); `cwd`/`gitBranch` are pulled out of it via
/// [`extract_json_string_field`]. A probe is dropped entirely when its id
/// fails [`crate::validate::claude_session_id`] or its mtime does not parse
/// as `i64` — in that case the following `@@L` line (if any) is also
/// ignored, since it belongs to the dropped probe. Any other line (login
/// banners, stray shell noise) is ignored.
pub fn parse_discover_output(stdout: &str) -> (Option<i64>, Vec<TranscriptProbe>) {
    let mut boot: Option<i64> = None;
    let mut probes: Vec<TranscriptProbe> = Vec::new();
    let mut current: Option<TranscriptProbe> = None;

    for line in stdout.lines() {
        if let Some(v) = line.strip_prefix("bootsec=") {
            boot = v.trim().parse::<i64>().ok();
        } else if let Some(v) = line.strip_prefix("bootraw=") {
            boot = parse_bootraw(v);
        } else if let Some(rest) = line.strip_prefix("@@F\t") {
            if let Some(p) = current.take() {
                probes.push(p);
            }
            let mut parts = rest.splitn(2, '\t');
            let mtime = parts.next().and_then(|s| s.trim().parse::<i64>().ok());
            let id = parts.next().unwrap_or("");
            current = match mtime {
                Some(mtime) if crate::validate::claude_session_id(id).is_ok() => {
                    Some(TranscriptProbe {
                        claude_session_id: id.to_string(),
                        mtime,
                        cwd: None,
                        git_branch: None,
                    })
                }
                _ => None,
            };
        } else if let Some(rest) = line.strip_prefix("@@L\t") {
            if let Some(p) = current.as_mut() {
                p.cwd = extract_json_string_field(rest, "\"cwd\":");
                p.git_branch = extract_json_string_field(rest, "\"gitBranch\":");
            }
        }
        // Anything else (login banners, stray shell noise) is ignored.
    }
    if let Some(p) = current.take() {
        probes.push(p);
    }
    (boot, probes)
}

/// One ranked candidate for lost-session restore: a transcript, deduplicated
/// by `cwd`, with a hint for how it relates to the host's last boot. Task 7
/// leaves `derived_tmux_name`/`project_id`/`worktree_id`/
/// `existing_session_id` as `None` — Task 8 (which has the store and the
/// fleet naming scheme) fills them in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LostCandidate {
    pub cwd: String,
    pub git_branch: Option<String>,
    pub claude_session_id: String,
    pub transcript_mtime: i64,
    pub derived_tmux_name: Option<String>,
    pub project_id: Option<i64>,
    pub worktree_id: Option<i64>,
    pub existing_session_id: Option<i64>,
    pub rank_hint: String,
}

/// `rank_hint` for one transcript's mtime against the host's boot epoch:
/// `"unknown"` when the boot epoch could not be read at all, `"after_boot"`
/// when the transcript was touched at or after boot, `"before_boot"` when it
/// was touched at most 24h before boot (a session that was very likely still
/// live right up to the reboot), else `"stale"`.
fn rank_hint_for(boot: Option<i64>, mtime: i64) -> &'static str {
    match boot {
        None => "unknown",
        Some(boot) => {
            if mtime >= boot {
                "after_boot"
            } else if boot - mtime <= 86_400 {
                "before_boot"
            } else {
                "stale"
            }
        }
    }
}

fn hint_order(hint: &str) -> u8 {
    match hint {
        "before_boot" => 0,
        "after_boot" => 1,
        "stale" => 2,
        _ => 3, // "unknown"
    }
}

/// Turn discovered transcripts into ranked, deduplicated candidates: probes
/// with no `cwd` are dropped outright (nothing to restore into), the rest
/// are grouped by `cwd` keeping only the one with the max `mtime` (a tie —
/// same `cwd`, same `mtime` — keeps the lexicographically larger session
/// id), and the result is ordered `before_boot`, then `after_boot`, then
/// `stale`, then `unknown`, newest (`mtime` desc) first within each group.
/// A `BTreeMap` keyed by `cwd` is used (not a `HashMap`) so the grouping
/// pass, and therefore any tie in the final sort, is deterministic.
pub fn rank_candidates(boot: Option<i64>, probes: Vec<TranscriptProbe>) -> Vec<LostCandidate> {
    let mut best: std::collections::BTreeMap<String, TranscriptProbe> =
        std::collections::BTreeMap::new();
    for p in probes {
        let Some(cwd) = p.cwd.clone() else {
            continue;
        };
        let keep = match best.get(&cwd) {
            None => true,
            Some(existing) => {
                (p.mtime, p.claude_session_id.as_str())
                    > (existing.mtime, existing.claude_session_id.as_str())
            }
        };
        if keep {
            best.insert(cwd, p);
        }
    }

    let mut candidates: Vec<LostCandidate> = best
        .into_iter()
        .map(|(cwd, p)| LostCandidate {
            cwd,
            git_branch: p.git_branch,
            claude_session_id: p.claude_session_id,
            transcript_mtime: p.mtime,
            derived_tmux_name: None,
            project_id: None,
            worktree_id: None,
            existing_session_id: None,
            rank_hint: rank_hint_for(boot, p.mtime).to_string(),
        })
        .collect();

    candidates.sort_by(|a, b| {
        hint_order(&a.rank_hint)
            .cmp(&hint_order(&b.rank_hint))
            .then(b.transcript_mtime.cmp(&a.transcript_mtime))
    });
    candidates
}

// ---- Task 8: live host + store enrichment ---------------------------------

#[derive(Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "DiscoverLostSessionsParams")]
pub struct DiscoverLostSessionsArgs {
    /// Host to scan.
    pub host_alias: String,
    /// Max transcripts to read, newest first. Default 50, max 500.
    #[serde(default)]
    pub limit: Option<i64>,
}

/// Wall clock for one host's discover-transcripts script. It reads up to
/// [`crate::tmux::discover_transcripts_script`]'s clamp of 500 transcripts,
/// each up to a 4 MiB `tail`, sequentially in one ssh round trip — generous
/// next to `HOST_PROBE_TIMEOUT` (30s, a whole reconcile pass across every
/// live tmux session) since this is a single on-demand call, not something
/// gating every host's sidebar refresh.
const DISCOVER_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

/// `limit` when the caller omits it entirely.
const DEFAULT_DISCOVER_LIMIT: usize = 50;

/// The deterministic tmux name `fill_session_name` would mint for a fresh
/// session in `owner/repo` at `worktree_key` (`"main"` for the repo root,
/// else a linked worktree's name — see `worktree_key_for_host`) — extracted
/// from `fill_session_name`'s deterministic branch so both callers derive the
/// exact same name from the exact same inputs. Does not consult the store
/// for a collision (unlike `fill_session_name`'s fallback-to-generated-pair
/// path): a discovery candidate's derived name is a hint for `new_session`'s
/// `name` argument, not a guarantee, and may already be taken by a second
/// session on the same worktree — the MCP tool description says so.
pub(crate) fn derive_tmux_name(owner: &str, repo: &str, worktree_key: &str) -> String {
    use crate::service::names::tmux_safe;
    let base = format!("dev-{owner}-{repo}");
    tmux_safe(&if worktree_key == "main" {
        base
    } else {
        format!("{base}--{worktree_key}")
    })
}

/// Run [`crate::tmux::discover_transcripts_script`] on `host_alias` via
/// `shell`, parse + rank its output ([`parse_discover_output`],
/// [`rank_candidates`]), then enrich each candidate from the store under one
/// short lock taken AFTER the ssh call returns (never held across an
/// `.await`): `existing_session_id` (a row — live or lost — on this host
/// whose `claude_session_id` matches), `project_id`
/// (`find_project_id_for_path`), `worktree_id` (the worktree row on this
/// host whose `path` equals the candidate's `cwd`, if any), and
/// `derived_tmux_name` (via [`derive_tmux_name`], only once `project_id` is
/// known — an orphan cwd with no project gets no name to restore into).
/// Mutates nothing. The test seam: production wires this to a real host via
/// [`discover_lost_sessions`]; tests inject a fake [`HostShell`].
pub(crate) async fn discover_lost_sessions_with(
    args: DiscoverLostSessionsArgs,
    store: &Mutex<Store>,
    shell: &dyn HostShell,
) -> Result<Vec<LostCandidate>, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    // `None` defaults to 50; an explicit value (including <= 0) is cast to
    // `usize` and handed straight to `discover_transcripts_script`, whose own
    // `.clamp(1, 500)` is the single source of truth for bounding it — a
    // negative `i64` wraps to a huge `usize` under the cast, which the clamp
    // pins to 500 (the max), and `0` clamps to 1. Not re-validated here on
    // purpose, so that one clamp never has a second copy to drift from.
    let limit: usize = match args.limit {
        Some(n) => n as usize,
        None => DEFAULT_DISCOVER_LIMIT,
    };
    let script = crate::tmux::discover_transcripts_script(limit);
    let stdout = shell.run_script(&args.host_alias, &script).await?;
    let (boot, probes) = parse_discover_output(&stdout);
    let mut candidates = rank_candidates(boot, probes);

    let s = lock(store)?;
    let sessions_on_host = s.list_sessions_for_host(&args.host_alias)?;
    let paths = HostPaths::for_host(&s, &args.host_alias);
    let projects = s.list_projects()?;
    let worktrees = s.list_worktrees_on_host(&args.host_alias)?;
    for c in &mut candidates {
        c.existing_session_id = sessions_on_host
            .iter()
            .find(|r| r.claude_session_id.as_deref() == Some(c.claude_session_id.as_str()))
            .map(|r| r.id);
        c.project_id = find_project_id_for_path(
            &projects,
            &args.host_alias,
            std::path::Path::new(&c.cwd),
            &paths,
        );
        c.worktree_id = worktrees.iter().find(|w| w.path == c.cwd).map(|w| w.id);
        c.derived_tmux_name = c.project_id.and_then(|pid| {
            let (owner, repo) = fetch_owner_repo(&s, pid).ok()?;
            let worktree_key = worktree_key_for_host(&c.cwd, &paths)?;
            Some(derive_tmux_name(&owner, &repo, &worktree_key))
        });
    }
    Ok(candidates)
}

/// Discover a host's lost-but-resumable Claude sessions: scans
/// `~/.claude/projects` on the host for transcripts fleet has no row for
/// (e.g. after a reboot before this fleet version) and ranks/enriches them —
/// see [`discover_lost_sessions_with`]. Read-only.
pub async fn discover_lost_sessions(
    args: DiscoverLostSessionsArgs,
    store: &Mutex<Store>,
    ssh: &Arc<SshClient>,
) -> Result<Vec<LostCandidate>, IpcError> {
    let shell = RealHostShell {
        ssh: Arc::clone(ssh),
        timeout: DISCOVER_TIMEOUT,
    };
    discover_lost_sessions_with(args, store, &shell).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(id: &str, mtime: i64, cwd: &str) -> TranscriptProbe {
        TranscriptProbe {
            claude_session_id: id.to_string(),
            mtime,
            cwd: Some(cwd.to_string()),
            git_branch: None,
        }
    }

    // ---- parse_discover_output -------------------------------------------------

    #[test]
    fn truncated_message_still_yields_cwd_and_branch() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let last_line = r#"{"parentUuid":"x","cwd":"/a/b \"q\"","sessionId":"abc","gitBranch":"feat/x","message":{"content":"abc"#;
        let stdout = format!("bootsec=1000\n@@F\t123\t{id}\n@@L\t{last_line}\n");

        let (boot, probes) = parse_discover_output(&stdout);

        assert_eq!(boot, Some(1000));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].claude_session_id, id);
        assert_eq!(probes[0].mtime, 123);
        assert_eq!(probes[0].cwd.as_deref(), Some("/a/b \"q\""));
        assert_eq!(probes[0].git_branch.as_deref(), Some("feat/x"));
    }

    #[test]
    fn bootraw_macos_format_is_parsed() {
        let (boot, probes) =
            parse_discover_output("bootraw={ sec = 1726000000, usec = 0 } Thu Sep 11 2025\n");
        assert_eq!(boot, Some(1_726_000_000));
        assert!(probes.is_empty());
    }

    #[test]
    fn bootraw_empty_when_sysctl_failed_is_unknown() {
        let (boot, _probes) = parse_discover_output("bootraw=\n");
        assert_eq!(boot, None);
    }

    #[test]
    fn invalid_session_id_drops_the_probe() {
        let stdout = "bootsec=1\n@@F\t100\tnot-a-uuid\n@@L\t{\"cwd\":\"/x\"}\n";
        let (boot, probes) = parse_discover_output(stdout);
        assert_eq!(boot, Some(1));
        assert!(probes.is_empty(), "{probes:?}");
    }

    #[test]
    fn unparseable_mtime_drops_the_probe() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout = format!("@@F\tNaN\t{id}\n@@L\t{{\"cwd\":\"/z\"}}\n");
        let (boot, probes) = parse_discover_output(&stdout);
        assert_eq!(boot, None);
        assert!(probes.is_empty(), "{probes:?}");
    }

    #[test]
    fn garbage_lines_are_ignored() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout =
            format!("some login banner\nbootsec=42\nnoise\n@@F\t7\t{id}\n@@L\t{{\"cwd\":\"/z\"}}\nmore noise\n");
        let (boot, probes) = parse_discover_output(&stdout);
        assert_eq!(boot, Some(42));
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cwd.as_deref(), Some("/z"));
    }

    #[test]
    fn missing_cwd_or_branch_line_is_none() {
        let id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let stdout = format!("@@F\t5\t{id}\n@@L\t\n");
        let (_boot, probes) = parse_discover_output(&stdout);
        assert_eq!(probes.len(), 1);
        assert_eq!(probes[0].cwd, None);
        assert_eq!(probes[0].git_branch, None);
    }

    // ---- rank_candidates ---------------------------------------------------

    #[test]
    fn duplicate_cwd_keeps_the_newest() {
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", 100, "/a"),
            probe("22222222-2222-2222-2222-222222222222", 200, "/a"),
            probe("00000000-0000-0000-0000-000000000000", 150, "/a"),
        ];
        let out = rank_candidates(Some(1_000_000), probes);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].claude_session_id,
            "22222222-2222-2222-2222-222222222222"
        );
        assert_eq!(out[0].transcript_mtime, 200);
    }

    #[test]
    fn duplicate_cwd_and_mtime_tie_breaks_on_larger_id() {
        let probes = vec![
            probe("aaaaaaaa-aaaa-aaaa-aaaa-aaaaaaaaaaaa", 100, "/a"),
            probe("bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb", 100, "/a"),
        ];
        let out = rank_candidates(Some(1_000_000), probes);
        assert_eq!(out.len(), 1);
        assert_eq!(
            out[0].claude_session_id,
            "bbbbbbbb-bbbb-bbbb-bbbb-bbbbbbbbbbbb"
        );
    }

    #[test]
    fn probes_without_cwd_are_dropped() {
        let mut p = probe("11111111-1111-1111-1111-111111111111", 100, "/a");
        p.cwd = None;
        let out = rank_candidates(Some(1_000_000), vec![p]);
        assert!(out.is_empty());
    }

    #[test]
    fn rank_hint_boundaries() {
        let boot = 1_000_000i64;
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", boot, "/after"),
            probe(
                "22222222-2222-2222-2222-222222222222",
                boot - 86_400,
                "/before-edge",
            ),
            probe(
                "33333333-3333-3333-3333-333333333333",
                boot - 86_401,
                "/stale-edge",
            ),
        ];
        let out = rank_candidates(Some(boot), probes);
        let hint_for = |cwd: &str| {
            out.iter()
                .find(|c| c.cwd == cwd)
                .unwrap_or_else(|| panic!("no candidate for {cwd}"))
                .rank_hint
                .clone()
        };
        assert_eq!(hint_for("/after"), "after_boot");
        assert_eq!(hint_for("/before-edge"), "before_boot");
        assert_eq!(hint_for("/stale-edge"), "stale");
    }

    #[test]
    fn ordering_across_groups() {
        let boot = 1_000_000i64;
        let probes = vec![
            probe(
                "11111111-1111-1111-1111-111111111111",
                boot - 90_000,
                "/stale-old",
            ),
            probe(
                "22222222-2222-2222-2222-222222222222",
                boot + 500,
                "/after-new",
            ),
            probe(
                "33333333-3333-3333-3333-333333333333",
                boot - 100,
                "/before-new",
            ),
            probe(
                "44444444-4444-4444-4444-444444444444",
                boot + 100,
                "/after-old",
            ),
            probe(
                "55555555-5555-5555-5555-555555555555",
                boot - 87_000,
                "/stale-new",
            ),
        ];
        let out = rank_candidates(Some(boot), probes);
        let order: Vec<&str> = out.iter().map(|c| c.cwd.as_str()).collect();
        assert_eq!(
            order,
            vec![
                "/before-new",
                "/after-new",
                "/after-old",
                "/stale-new",
                "/stale-old",
            ]
        );
    }

    #[test]
    fn none_boot_marks_everything_unknown_ordered_by_mtime() {
        let probes = vec![
            probe("11111111-1111-1111-1111-111111111111", 100, "/a"),
            probe("22222222-2222-2222-2222-222222222222", 300, "/b"),
            probe("33333333-3333-3333-3333-333333333333", 200, "/c"),
        ];
        let out = rank_candidates(None, probes);
        assert!(out.iter().all(|c| c.rank_hint == "unknown"), "{out:?}");
        let order: Vec<&str> = out.iter().map(|c| c.cwd.as_str()).collect();
        assert_eq!(order, vec!["/b", "/c", "/a"]);
    }

    // ---- derive_tmux_name ---------------------------------------------------

    #[test]
    fn derive_tmux_name_matches_fill_session_names_deterministic_branch() {
        assert_eq!(derive_tmux_name("o", "r", "main"), "dev-o-r");
        assert_eq!(derive_tmux_name("o", "r", "feat-x"), "dev-o-r--feat-x");
        // tmux_safe still runs: a `.` in an owner/repo is not the norm, but
        // must come out the same way it would through fill_session_name.
        assert_eq!(derive_tmux_name("o.rg", "r", "main"), "dev-o-rg-r");
    }

    // ---- discover_lost_sessions_with ----------------------------------------

    /// A shell that always answers with the same canned stdout, regardless of
    /// host/script — enough to drive `discover_lost_sessions_with` without a
    /// real host.
    struct CannedDiscoverShell {
        stdout: String,
    }

    #[async_trait::async_trait]
    impl HostShell for CannedDiscoverShell {
        async fn run_script(&self, _host: &str, _script: &str) -> Result<String, IpcError> {
            Ok(self.stdout.clone())
        }
    }

    fn session_events_count(s: &Store, session_id: i64) -> usize {
        s.list_session_events(session_id, 1000).unwrap().len()
    }

    #[tokio::test]
    async fn enriches_candidates_from_the_store_and_writes_nothing() {
        let claude_id = "44366faf-ae97-426a-91cd-beaf3c74f1d7";
        let cwd = "/home/x/projects/github.com/o/r/.worktrees/feat";
        let stdout = format!(
            "bootsec=1000\n@@F\t2000\t{claude_id}\n@@L\t{{\"cwd\":\"{cwd}\",\"gitBranch\":\"feat\"}}\n"
        );

        let s = Store::open_in_memory().expect("open");
        s.upsert_host("h").unwrap();
        let pid = s
            .upsert_project("o", "r", "/home/x/projects/github.com/o/r")
            .unwrap();
        let wid = s
            .upsert_worktree_on("h", pid, "feat", cwd, Some("feat"))
            .unwrap();
        // An existing lost row whose claude_session_id matches the candidate
        // — `existing_session_id` must resolve to it.
        let existing = s
            .upsert_session(
                "dev-o-r--feat",
                "h",
                Some(pid),
                Some(wid),
                1,
                1,
                "running",
                None,
            )
            .unwrap();
        s.set_claude_session_id(existing, claude_id).unwrap();
        s.mark_host_sessions_lost("h", "host_reboot", &[], 500, 0)
            .unwrap();

        let store = Mutex::new(s);
        let before = lock(&store).unwrap().list_sessions_for_host("h").unwrap();
        let before_events = session_events_count(&lock(&store).unwrap(), existing);

        let shell = CannedDiscoverShell { stdout };
        let candidates = discover_lost_sessions_with(
            DiscoverLostSessionsArgs {
                host_alias: "h".to_string(),
                limit: None,
            },
            &store,
            &shell,
        )
        .await
        .unwrap();

        assert_eq!(candidates.len(), 1);
        let c = &candidates[0];
        assert_eq!(c.claude_session_id, claude_id);
        assert_eq!(c.cwd, cwd);
        assert_eq!(c.project_id, Some(pid));
        assert_eq!(c.worktree_id, Some(wid));
        assert_eq!(c.derived_tmux_name.as_deref(), Some("dev-o-r--feat"));
        assert_eq!(c.existing_session_id, Some(existing));

        // Read-only: the store's session rows and event count survive
        // untouched.
        let after = lock(&store).unwrap().list_sessions_for_host("h").unwrap();
        assert_eq!(before, after, "discover must not write anything");
        let after_events = session_events_count(&lock(&store).unwrap(), existing);
        assert_eq!(
            before_events, after_events,
            "discover must not insert events"
        );
    }

    #[tokio::test]
    async fn a_shell_error_propagates() {
        let store = Mutex::new(Store::open_in_memory().expect("open"));
        lock(&store).unwrap().upsert_host("h").unwrap();

        let err = discover_lost_sessions_with(
            DiscoverLostSessionsArgs {
                host_alias: "h".to_string(),
                limit: None,
            },
            &store,
            &NoHostShell,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, codes::E_SHELL);
    }
}
