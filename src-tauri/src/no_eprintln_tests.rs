//! Guard (Track H): production code logs through `tracing`, so every line
//! reaches the redacting file layer in `logging.rs`, never through the print
//! family (`eprintln!`, `eprint!`, `println!`, `print!`, `dbg!`). Test code
//! may still print. There is no allowlist: any production print call fails.
//!
//! Every `.rs` file under `src/` is scanned, skipping test-only code:
//! - files declared `#[cfg(test)] mod name;` (e.g. `fleet_e2e_tests.rs`,
//!   `ssh_fake.rs`, this file);
//! - an inline `#[cfg(test)] mod name { … }`, up to its closing brace, found
//!   as the first later line that is exactly the `mod` line's indentation
//!   followed by `}`. rustfmt puts it there and CI enforces `cargo fmt
//!   --check`. Production code after the module is scanned again;
//! - comment lines.
//!
//! Why not count braces: braces inside string literals, raw strings above all
//! (`r#"{"hooks": {"Stop": ["#`, `"exec ${SHELL"`), made the count end a
//! module LATE, so production code after it was silently skipped (a false
//! pass). An inline test module whose closing brace is not where rustfmt puts
//! it (an unformatted file) is reported as an error instead of guessed.
//!
//! The scan is line-based and deliberately conservative: it may fail on a line
//! that is really test-only or not a call at all, never the other way round.
//! Known false-fail cases (rewrite the line, or move it into a test module):
//! - `#[cfg(any(test, …))]` / `#[cfg(all(test, …))]`: only a bare
//!   `#[cfg(test)]` marks test code;
//! - a trailing comment on the `#[cfg(test)]` line (`#[cfg(test)] // why`);
//! - a print macro inside a `/* … */` block comment, or after code on the same
//!   line (`foo(); // eprintln!(…)`): only whole `//` lines are skipped.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

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

/// Index of the line that closes the inline module opened on `mod_idx`: the
/// first later line that is exactly that line's indentation plus `}`. `None`
/// when the file ends first.
fn module_end(lines: &[&str], mod_idx: usize) -> Option<usize> {
    let indent: String = lines[mod_idx]
        .chars()
        .take_while(|c| c.is_whitespace())
        .collect();
    let close = format!("{indent}}}");
    lines
        .iter()
        .enumerate()
        .skip(mod_idx + 1)
        .find(|(_, l)| l.trim_end() == close)
        .map(|(k, _)| k)
}

/// What one file's scan found.
#[derive(Debug, Default, PartialEq)]
struct Scan {
    /// 1-based line numbers of production print calls.
    hits: Vec<usize>,
    /// Test-only module files it declares (`#[cfg(test)] mod name;`).
    test_mods: Vec<String>,
    /// 1-based lines of inline test modules whose closing brace was not found.
    unterminated: Vec<usize>,
}

fn scan(text: &str) -> Scan {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Scan::default();
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
                    out.test_mods.push(name.trim().to_string());
                } else if rest.ends_with('}') {
                    // A one-line `mod x { … }`: only that line is test code.
                    i = item_idx + 1;
                    continue;
                } else if rest.ends_with('{') {
                    // Inline test module: skip to its closing brace, then keep
                    // scanning whatever production code follows it.
                    match module_end(&lines, item_idx) {
                        Some(end) => {
                            i = end + 1;
                            continue;
                        }
                        None => {
                            out.unterminated.push(item_idx + 1);
                            break;
                        }
                    }
                }
            }
            i += 1;
            continue;
        }
        if !t.starts_with("//") && calls_print(t) {
            out.hits.push(i + 1);
        }
        i += 1;
    }
    out
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
        let result = scan(&text);
        // `mod name;` in lib.rs / main.rs / mod.rs resolves next to the file;
        // in `foo.rs` it resolves under `foo/`.
        let base = match file.file_name().and_then(|n| n.to_str()) {
            Some("lib.rs" | "main.rs" | "mod.rs") => file.parent().expect("parent").to_path_buf(),
            _ => file.with_extension(""),
        };
        for m in &result.test_mods {
            test_only.insert(base.join(format!("{m}.rs")));
            test_only.insert(base.join(m).join("mod.rs"));
        }
        scanned.push((file.clone(), result));
    }

    let mut offenders = Vec::new();
    let mut unterminated = Vec::new();
    for (file, result) in scanned {
        if test_only.contains(&file) {
            continue;
        }
        let rel = file
            .strip_prefix(&src)
            .expect("under src")
            .to_string_lossy()
            .replace('\\', "/");
        if !result.hits.is_empty() {
            offenders.push(format!("{rel}: lines {:?}", result.hits));
        }
        if !result.unterminated.is_empty() {
            unterminated.push(format!("{rel}: lines {:?}", result.unterminated));
        }
    }
    assert!(
        unterminated.is_empty(),
        "inline #[cfg(test)] module without its closing `}}` at the `mod` line's \
         indentation (run cargo fmt); the rest of the file could not be checked:\n{}",
        unterminated.join("\n")
    );
    assert!(
        offenders.is_empty(),
        "production print macro ({}) found; use tracing::{{error,warn,info,debug}}! so it \
         reaches the redacting log layer:\n{}",
        needles().join(", "),
        offenders.join("\n")
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
    let r = scan(&text);
    assert_eq!(
        r.hits,
        vec![1, 7],
        "production calls before and after a cfg(test) fn"
    );
    assert_eq!(r.test_mods, vec!["helper".to_string()]);
    assert!(r.unterminated.is_empty());
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
    assert_eq!(
        scan(&text).hits,
        vec![7, 14],
        "only the two production calls"
    );
}

#[test]
fn braces_in_raw_strings_do_not_end_a_test_module_late() {
    // Brace counting would still be "inside" the module after its closing
    // `}` (the raw string opens two braces it never closes) and so would skip
    // the production call below it: the false pass the indentation rule fixes.
    let n = needles()[0];
    let text = format!(
        "#[cfg(test)]\n\
         mod tests {{\n\
         \x20   const HOOKS: &str = r#\"{{\"hooks\": {{\"Stop\": [\"#;\n\
         \x20   const CMD: &str = \"exec ${{SHELL\";\n\
         \x20   fn t() {{ {n}(\"test\"); }}\n\
         }}\n\
         fn prod() {{ {n}(\"prod\"); }}\n"
    );
    let r = scan(&text);
    assert_eq!(r.hits, vec![7]);
    assert!(r.unterminated.is_empty());
}

#[test]
fn an_unterminated_test_module_is_reported_not_guessed() {
    let n = needles()[0];
    let text = format!(
        "fn a() {{}}\n\
         #[cfg(test)]\n\
         mod tests {{\n\
         \x20   fn t() {{ {n}(\"test\"); }}\n\
         \x20   }}\n"
    );
    let r = scan(&text);
    assert_eq!(r.unterminated, vec![3]);
    assert!(r.hits.is_empty());
}

#[test]
fn every_print_macro_is_caught_and_identifiers_are_not() {
    for n in needles() {
        let r = scan(&format!("fn a() {{ {n}(\"x\"); }}\n"));
        assert_eq!(r.hits, vec![1], "{n} must be caught");
    }
    let my_dbg = format!("my_{}", needles()[4]);
    let r = scan(&format!("fn a() {{ {my_dbg}(x); }}\n"));
    assert!(r.hits.is_empty(), "{my_dbg} is not a print macro");
}
