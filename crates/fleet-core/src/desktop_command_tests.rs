//! Source checks on desktop commands (`src-tauri/src/commands/…`) that
//! this crate can run without the desktop's system libraries.
//!
//! ## Off the main thread
//!
//! A plain `#[tauri::command] pub fn` runs on the thread that called it, and
//! on macOS that is the main thread: while it blocks, the whole window is
//! frozen (CLAUDE.md, Conventions). Each command below reaches the OS
//! keychain, the disk or the store, any of which can stall for as long as a
//! keychain prompt sits unanswered or a network mount takes to answer, so
//! each is declared `#[tauri::command(async)]` (or `async fn`), which moves it
//! off the main thread.
//!
//! A command that takes the store needs no entry: every command under
//! `src-tauri/src/commands/` is checked for that (review r18). The list is
//! the others found reaching the keychain or the disk; a command that grows
//! such a call joins it.

/// `(file, command)` pairs, the file relative to the repository root.
const OFF_MAIN_THREAD: &[(&str, &str)] = &[
    // `logic::status` / `stranded_token` / `disconnect` read or clear the
    // keychain through the raw `OsTokenStore`; only the startup resolve is
    // wrapped in `BoundedTokenStore`'s wait.
    ("src-tauri/src/commands/hub.rs", "hub_status"),
    ("src-tauri/src/commands/hub.rs", "hub_stranded_token"),
    ("src-tauri/src/commands/hub.rs", "hub_disconnect"),
    // A dropped file can sit on a stalled SMB/NFS mount or be an iCloud
    // placeholder that has to download before `metadata` returns.
    ("src-tauri/src/commands/upload.rs", "attachment_preview"),
    ("src-tauri/src/commands/upload.rs", "attachment_describe"),
    // Reads and rewrites `~/.claude/settings.json` (it also takes the store,
    // which `no_desktop_command_locks_the_store_on_the_main_thread` covers
    // for every command).
    ("src-tauri/src/commands/mcp.rs", "install_fleet_hook"),
];

/// The attribute line directly above `pub fn <name>(` / `pub async fn <name>(`.
fn declaration(src: &str, name: &str) -> Option<(String, bool)> {
    let lines: Vec<&str> = src.lines().collect();
    let at = lines.iter().position(|l| {
        let l = l.trim_start();
        l.starts_with(&format!("pub fn {name}(")) || l.starts_with(&format!("pub async fn {name}("))
    })?;
    let is_async_fn = lines[at].trim_start().starts_with("pub async fn");
    let attr = lines[..at]
        .iter()
        .rev()
        .map(|l| l.trim())
        .find(|l| l.starts_with("#[tauri::command"))?;
    Some((attr.to_string(), is_async_fn))
}

#[test]
fn blocking_desktop_commands_stay_off_the_main_thread() {
    let mut sync = Vec::new();
    for (file, name) in OFF_MAIN_THREAD {
        let src = crate::repo_files::read(file);
        let (attr, is_async_fn) = declaration(&src, name)
            .unwrap_or_else(|| panic!("{file}: no `#[tauri::command]` fn {name}"));
        if !is_async_fn && attr != "#[tauri::command(async)]" {
            sync.push(format!("{file}: {name} ({attr})"));
        }
    }
    assert!(
        sync.is_empty(),
        "these commands block on the keychain, the disk or the store, and a \
         sync command runs on the macOS main thread; declare them \
         `#[tauri::command(async)]`:\n  {}",
        sync.join("\n  ")
    );
}

/// The plain `#[tauri::command] pub fn`s in `src` whose signature takes the
/// store: `(name, signature)`.
fn sync_commands_taking_the_store(src: &str) -> Vec<String> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, l) in lines.iter().enumerate() {
        if l.trim() != "#[tauri::command]" {
            continue;
        }
        let Some(decl) = lines.get(i + 1).map(|d| d.trim_start()) else {
            continue;
        };
        let Some(rest) = decl.strip_prefix("pub fn ") else {
            continue;
        };
        let sig: String = lines[i + 1..]
            .iter()
            .take_while(|s| !s.trim_end().ends_with('{'))
            .chain(lines[i + 1..].iter().find(|s| s.trim_end().ends_with('{')))
            .copied()
            .collect();
        if sig.contains("Store>") {
            out.push(rest.split('(').next().unwrap_or(rest).to_string());
        }
    }
    out
}

/// Review r18: every desktop command that takes the store runs off the main
/// thread. The store is a `std::sync::Mutex` that a reconcile pass or a slow
/// statement can hold, and a sync command waiting on it freezes the window
/// on macOS. Every file under `src-tauri/src/commands/` is checked, so a new
/// command cannot slip by the way it could a list.
#[test]
fn no_desktop_command_locks_the_store_on_the_main_thread() {
    let dir = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../src-tauri/src/commands");
    let mut files = Vec::new();
    let mut stack = vec![dir];
    while let Some(d) = stack.pop() {
        for e in std::fs::read_dir(&d).unwrap_or_else(|e| panic!("read {}: {e}", d.display())) {
            let p = e.unwrap().path();
            if p.is_dir() {
                stack.push(p);
            } else if p.extension().is_some_and(|x| x == "rs") {
                files.push(p);
            }
        }
    }
    assert!(files.len() > 20, "found the commands: {}", files.len());
    let mut sync = Vec::new();
    for f in files {
        let src = std::fs::read_to_string(&f).unwrap();
        for name in sync_commands_taking_the_store(&src) {
            sync.push(format!("{}: {name}", f.display()));
        }
    }
    sync.sort();
    assert!(
        sync.is_empty(),
        "these commands take the store, and a sync command runs on the macOS \
         main thread; declare them `#[tauri::command(async)]`:\n  {}",
        sync.join("\n  ")
    );
}

#[test]
fn sync_commands_taking_the_store_reads_the_signature() {
    let src = "#[tauri::command]\npub fn a(\n    store: State<'_, Arc<Mutex<Store>>>,\n) -> Result<(), E> {\n}\n\n\
               #[tauri::command(async)]\npub fn b(store: State<'_, Arc<Mutex<Store>>>) {\n}\n\n\
               #[tauri::command]\npub fn c(x: u8) -> u8 {\n}\n\n\
               #[tauri::command]\npub async fn d(store: State<'_, Arc<Mutex<Store>>>) {\n}\n";
    assert_eq!(sync_commands_taking_the_store(src), vec!["a".to_string()]);
}

#[test]
fn declaration_reads_the_attribute_above_the_fn() {
    let src = "#[tauri::command]\npub fn a(x: u8) {}\n\n\
               #[tauri::command(async)]\npub fn b() {}\n\n\
               /// doc\n#[tauri::command]\npub async fn c() {}\n";
    assert_eq!(
        declaration(src, "a"),
        Some(("#[tauri::command]".into(), false))
    );
    assert_eq!(
        declaration(src, "b"),
        Some(("#[tauri::command(async)]".into(), false))
    );
    assert_eq!(
        declaration(src, "c"),
        Some(("#[tauri::command]".into(), true))
    );
    assert_eq!(declaration(src, "d"), None);
}

/// Commands that read a local file whose path arrives from the webview. SEC-9:
/// the webview never names a file this process opens — the path must be one
/// the user picked (in a Rust-side picker) or dropped, i.e. on the
/// `UploadAllowList`.
const READS_A_WEBVIEW_PATH: &[(&str, &str)] =
    &[("src-tauri/src/commands/assets.rs", "catalog_add_resource")];

#[test]
fn a_command_that_reads_a_webview_path_checks_the_allow_list() {
    for (file, name) in READS_A_WEBVIEW_PATH {
        let src = crate::repo_files::read(file);
        let start = src
            .find(&format!("fn {name}("))
            .unwrap_or_else(|| panic!("{file}: no fn {name}"));
        let rest = &src[start..];
        let body = &rest[..rest.find("#[tauri::command").unwrap_or(rest.len())];
        assert!(
            body.contains("check_paths_allowed(") && body.contains(".consume("),
            "{file}: {name} reads a path the webview sent without checking and \
             consuming the upload allow-list (SEC-9):\n{body}"
        );
    }
}
