mod commands;
mod ipc_error;
mod store;

use directories::ProjectDirs;
use std::sync::Mutex;
use store::Store;

fn appdata_db_path() -> std::path::PathBuf {
    let dirs = ProjectDirs::from("sk", "rlt", "claude-fleet")
        .expect("could not resolve platform appdata dir");
    let dir = dirs.data_dir();
    std::fs::create_dir_all(dir).unwrap_or_else(|e| panic!("create appdata dir {dir:?}: {e}"));
    dir.join("state.db")
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let db_path = appdata_db_path();
    let store = Store::open(&db_path).expect("open store");

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .manage(Mutex::new(store))
        .invoke_handler(tauri::generate_handler![commands::health::health_check])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
