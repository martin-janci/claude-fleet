use crate::store::Store;
use serde::Serialize;
use std::sync::Mutex;

#[derive(Serialize)]
pub struct Health {
    pub version: String,
    pub db_ready: bool,
    pub schema_version: i64,
}

pub fn health_from_store(s: &Store) -> Health {
    // TODO(T3): once IpcError exists, surface the failure reason here
    // instead of silently falling back to schema_version=0 / db_ready=false.
    let schema_version = s.schema_version().unwrap_or(0);
    Health {
        version: env!("CARGO_PKG_VERSION").to_string(),
        db_ready: schema_version >= 1,
        schema_version,
    }
}

pub fn health_check(store: &Mutex<Store>) -> Health {
    // A poisoned store mutex IS an unhealthy state — report db_ready=false
    // rather than panicking the command (which the old `.expect` did).
    match store.lock() {
        Ok(s) => health_from_store(&s),
        Err(_) => Health {
            version: env!("CARGO_PKG_VERSION").to_string(),
            db_ready: false,
            schema_version: 0,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[test]
    fn health_from_store_reports_version_db_ready_and_schema() {
        let store = Mutex::new(Store::open_in_memory().expect("in-memory store"));
        let s = store.lock().unwrap();
        let h = health_from_store(&s);
        assert_eq!(h.version, env!("CARGO_PKG_VERSION"));
        assert!(h.db_ready);
        assert_eq!(h.schema_version, 12);
    }
}
