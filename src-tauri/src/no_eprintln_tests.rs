//! Guard (Track H): production code logs through `tracing`, so every line
//! reaches the redacting file layer in `logging.rs`, never through the print
//! family (`eprintln!`, `eprint!`, `println!`, `print!`, `dbg!`). Test code
//! may still print.
//!
//! Every `.rs` file under `src/` is scanned, skipping test-only code:
//! - files declared `#[cfg(test)] mod name;` (e.g. `fleet_e2e_tests.rs`,
//!   `ssh_fake.rs`, this file);
//! - an inline `#[cfg(test)] mod name { … }` up to its closing brace. Brace
//!   depth is tracked, so production code after the module is still scanned;
//! - comment lines.
//!
//! The scan is line-based and deliberately conservative: it may fail on a line
//! that is really test-only or not a call at all, never the other way round.
//! Known false-fail cases (rewrite the line, or move it into a test module):
//! - `#[cfg(any(test, …))]` / `#[cfg(all(test, …))]`: only a bare
//!   `#[cfg(test)]` marks test code;
//! - a trailing comment on the `#[cfg(test)]` line (`#[cfg(test)] // why`);
//! - a print macro inside a `/* … */` block comment, or after code on the same
//!   line (`foo(); // eprintln!(…)`): only whole `//` lines are skipped;
//! - an unbalanced `{` / `}` in a string or char literal inside a test module,
//!   which can end the skipped region early.
//!
//! Files that still carry production print calls are listed in
//! [`NOT_YET_SWEPT`]. The test also fails on a stale entry, so the list can
//! only get shorter.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Files (relative to `src/`) whose production print calls have not been
/// converted to `tracing` yet (eprintln sweep part B2: each is being edited
/// by other open work). Remove an entry once its file is clean.
const NOT_YET_SWEPT: &[&str] = &[
    "lib.rs",
    "mcp/tools.rs",
    "service/move_session.rs",
    "service/projects.rs",
    "service/sessions.rs",
    "store.rs",
];

/// The print family, built so this file never contains the names literally.
fn needles() -> [&'static str; 5] {
    [
        concat!("eprint", "ln!"),
        concat!("eprint", "!"),
        concat!("print", "ln!"),
        concat!("print", "!"),
        concat!("db", "g!"),
    ]
}

/// True when `line` invokes one of the print macros: a macro name not
/// preceded by an identifier character (so `my_dbg!` is not a `dbg!`).
fn calls_print(line: &str) -> bool {
    needles().iter().any(|n| {
        line.match_indices(n).any(|(i, _)| {
            line[..i]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
        })
    })
}

/// Net `{` minus `}` on a line (literals are not parsed; see the module docs).
fn brace_delta(line: &str) -> i64 {
    line.chars().fold(0, |d, c| match c {
        '{' => d + 1,
        '}' => d - 1,
        _ => d,
    })
}

/// Scan one file: the 1-based line numbers of production print calls, and
/// the names of the test-only module files it declares
/// (`#[cfg(test)] mod name;`).
fn scan(text: &str) -> (Vec<usize>, Vec<String>) {
    let lines: Vec<&str> = text.lines().collect();
    let mut hits = Vec::new();
    let mut test_mods = Vec::new();
    let mut i = 0;
    while i < lines.len() {
        let t = lines[i].trim();
        if let Some(after) = t.strip_prefix("#[cfg(test)]") {
            // The item the attribute applies to: the rest of this line, or the
            // next line that is neither blank nor another attribute.
            let found = if after.trim().is_empty() {
                lines
                    .iter()
                    .enumerate()
                    .skip(i + 1)
                    .map(|(k, l)| (k, l.trim()))
                    .find(|(_, l)| !l.is_empty() && !l.starts_with("#["))
            } else {
                Some((i, after.trim()))
            };
            let Some((item_idx, item)) = found else {
                break;
            };
            let decl = item
                .strip_prefix("pub(crate) ")
                .or_else(|| item.strip_prefix("pub "))
                .unwrap_or(item);
            if let Some(rest) = decl.strip_prefix("mod ") {
                let rest = rest.trim_end();
                if let Some(name) = rest.strip_suffix(';') {
                    test_mods.push(name.trim().to_string());
                } else if rest.ends_with('{') {
                    // Inline test module: skip to its closing brace, then keep
                    // scanning whatever production code follows it.
                    let mut depth = brace_delta(item);
                    let mut k = item_idx + 1;
                    while depth > 0 && k < lines.len() {
                        depth += brace_delta(lines[k]);
                        k += 1;
                    }
                    i = k;
                    continue;
                }
            }
            i += 1;
            continue;
        }
        if !t.starts_with("//") && calls_print(t) {
            hits.push(i + 1);
        }
        i += 1;
    }
    (hits, test_mods)
}

#[test]
fn production_code_does_not_use_eprintln() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rs_files(&src, &mut files);
    files.sort();

    let mut test_only: BTreeSet<PathBuf> = BTreeSet::new();
    let mut scanned = Vec::new();
    for file in &files {
        let text = std::fs::read_to_string(file).expect("read source file");
        let (hits, mods) = scan(&text);
        // `mod name;` in lib.rs / main.rs / mod.rs resolves next to the file;
        // in `foo.rs` it resolves under `foo/`.
        let base = match file.file_name().and_then(|n| n.to_str()) {
            Some("lib.rs" | "main.rs" | "mod.rs") => file.parent().expect("parent").to_path_buf(),
            _ => file.with_extension(""),
        };
        for m in mods {
            test_only.insert(base.join(format!("{m}.rs")));
            test_only.insert(base.join(&m).join("mod.rs"));
        }
        scanned.push((file.clone(), hits));
    }

    let mut offenders = Vec::new();
    let mut stale = Vec::new();
    for (file, hits) in scanned {
        if test_only.contains(&file) {
            continue;
        }
        let rel = file
            .strip_prefix(&src)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        let allowed = NOT_YET_SWEPT.contains(&rel.as_str());
        match (hits.is_empty(), allowed) {
            (false, false) => offenders.push(format!("{rel}: lines {hits:?}")),
            (true, true) => stale.push(rel),
            _ => {}
        }
    }
    assert!(
        offenders.is_empty(),
        "production print macro ({}) found; use tracing::{{error,warn,info,debug}}! so it \
         reaches the redacting log layer:\n{}",
        needles().join(", "),
        offenders.join("\n")
    );
    assert!(
        stale.is_empty(),
        "these files no longer have a production print call; drop them from NOT_YET_SWEPT: {stale:?}"
    );
}

fn rs_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).expect("read src dir") {
        let path = entry.expect("dir entry").path();
        if path.is_dir() {
            rs_files(&path, out);
        } else if path.extension().is_some_and(|x| x == "rs") {
            out.push(path);
        }
    }
}

#[test]
fn scan_ignores_comments_and_test_code_but_not_cfg_test_items() {
    let n = needles()[0];
    let text = format!(
        "fn a() {{ {n}(\"x\"); }}\n\
         // {n} in a comment\n\
         #[cfg(test)]\n\
         mod helper;\n\
         #[cfg(test)]\n\
         pub fn only_in_tests() {{}}\n\
         fn b() {{ {n}(\"y\"); }}\n\
         #[cfg(test)]\n\
         mod tests {{\n    fn c() {{ {n}(\"z\"); }}\n}}\n"
    );
    let (hits, mods) = scan(&text);
    assert_eq!(
        hits,
        vec![1, 7],
        "production calls before and after a cfg(test) fn"
    );
    assert_eq!(mods, vec!["helper".to_string()]);
}

#[test]
fn an_inline_test_module_above_production_code_does_not_hide_it() {
    let n = needles()[0];
    let text = format!(
        "#[cfg(test)]\n\
         mod early {{\n\
         \x20   fn t() {{\n\
         \x20       if true {{ {n}(\"test\"); }}\n\
         \x20   }}\n\
         }}\n\
         fn prod() {{ {n}(\"prod\"); }}\n\
         #[cfg(test)] mod one_line {{ fn t() {{ {n}(\"t\"); }} }}\n\
         #[cfg(test)]\n\
         #[allow(dead_code)]\n\
         mod tail {{\n    fn t() {{ {n}(\"t\"); }}\n}}\n\
         fn after_tail() {{ {n}(\"prod 2\"); }}\n"
    );
    let (hits, _) = scan(&text);
    assert_eq!(hits, vec![7, 14], "only the two production calls");
}

#[test]
fn every_print_macro_is_caught_and_identifiers_are_not() {
    for n in needles() {
        let (hits, _) = scan(&format!("fn a() {{ {n}(\"x\"); }}\n"));
        assert_eq!(hits, vec![1], "{n} must be caught");
    }
    let my_dbg = format!("my_{}", needles()[4]);
    let (hits, _) = scan(&format!("fn a() {{ {my_dbg}(x); }}\n"));
    assert!(hits.is_empty(), "{my_dbg} is not a print macro");
}
