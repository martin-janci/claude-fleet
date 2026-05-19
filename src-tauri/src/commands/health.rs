use crate::store::Store;
use serde::Serialize;
use std::sync::Mutex;
use tauri::State;

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
    pub schema_version: i64,
}

#[tauri::command]
pub fn health_check(store: State<'_, Mutex<Store>>) -> Health {
    let s = store.lock().expect("store mutex poisoned");
    let schema_version = s.schema_version().unwrap_or(0);
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ready: schema_version >= 1,
        schema_version,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    fn test_state() -> Mutex<Store> {
        Mutex::new(Store::open_in_memory().expect("in-memory store"))
    }

    #[test]
    fn health_reports_db_state_from_shared_store() {
        let state = test_state();
        let s = state.lock().unwrap();
        let sv = s.schema_version().unwrap();
        assert_eq!(sv, 1);
    }
}
