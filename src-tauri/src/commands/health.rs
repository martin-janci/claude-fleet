use serde::Serialize;

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
}

#[tauri::command]
pub fn health_check() -> Health {
    let store_ok = crate::store::Store::open_in_memory().is_ok();
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ready: store_ok,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn health_check_reports_version_and_db_ready() {
        let h = health_check();
        assert_eq!(h.version, env!("CARGO_PKG_VERSION"));
        assert!(h.db_ready);
    }
}
