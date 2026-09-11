//! Guard (Track H): production code logs through `tracing`, so every line
//! reaches the redacting file layer in `logging.rs`, never through a bare
//! `eprintln!`. Test code may still print.
//!
//! Every `.rs` file under `src/` is scanned, skipping test-only code:
//! - files declared `#[cfg(test)] mod name;` (e.g. `fleet_e2e_tests.rs`,
//!   `ssh_fake.rs`, this file);
//! - everything from a `#[cfg(test)]` that opens an inline `mod … {` to the
//!   end of that file (test modules sit at the end of their file);
//! - comment lines.
//!
//! Files that still carry production `eprintln!` calls are listed in
//! [`NOT_YET_SWEPT`]. Part B of the sweep shrinks that list to empty: the test
//! also fails on a stale entry, so the list can only get shorter.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

/// Files (relative to `src/`) whose production `eprintln!` calls have not
/// been converted to `tracing` yet. Each is being edited by another open PR,
/// so it is swept in part B. Remove an entry once its file is clean.
const NOT_YET_SWEPT: &[&str] = &[
    "commands/mcp.rs",
    "lib.rs",
    "mcp/hooks.rs",
    "mcp/tools.rs",
    "service/bg_sessions.rs",
    "service/move_session.rs",
    "service/projects.rs",
    "service/sessions.rs",
    "ssh.rs",
    "store.rs",
];

/// The macro name, built so this file never contains it literally.
fn needle() -> String {
    concat!("eprint", "ln!").to_string()
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

/// Scan one file: the 1-based line numbers of production `eprintln!` calls,
/// and the names of the test-only module files it declares
/// (`#[cfg(test)] mod name;`).
fn scan(text: &str) -> (Vec<usize>, Vec<String>) {
    let needle = needle();
    let lines: Vec<&str> = text.lines().collect();
    let mut hits = Vec::new();
    let mut test_mods = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim();
        if let Some(after) = t.strip_prefix("#[cfg(test)]") {
            // The item the attribute applies to: the rest of this line, or the
            // next line that is neither blank nor another attribute.
            let item = if after.trim().is_empty() {
                lines[i + 1..]
                    .iter()
                    .map(|l| l.trim())
                    .find(|l| !l.is_empty() && !l.starts_with("#["))
                    .unwrap_or("")
            } else {
                after.trim()
            };
            let decl = item
                .strip_prefix("pub(crate) ")
                .or_else(|| item.strip_prefix("pub "))
                .unwrap_or(item);
            if let Some(rest) = decl.strip_prefix("mod ") {
                if let Some(name) = rest.trim_end().strip_suffix(';') {
                    test_mods.push(name.trim().to_string());
                } else if rest.trim_end().ends_with('{') {
                    break; // an inline test module: the rest of the file is test code
                }
            }
            continue;
        }
        if !t.starts_with("//") && t.contains(&needle) {
            hits.push(i + 1);
        }
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
        "production {} found; use tracing::{{error,warn,info,debug}}! so it reaches the redacting log layer:\n{}",
        needle(),
        offenders.join("\n")
    );
    assert!(
        stale.is_empty(),
        "these files no longer have a production {}; drop them from NOT_YET_SWEPT: {stale:?}",
        needle()
    );
}

#[test]
fn scan_ignores_comments_and_test_code_but_not_cfg_test_items() {
    let n = needle();
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
