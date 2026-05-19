use serde::Serialize;
use std::fmt;

#[derive(Debug, Serialize)]
#[allow(dead_code)]
pub struct IpcError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[allow(dead_code)]
impl IpcError {
    pub fn new(code: &str, message: impl Into<String>) -> Self {
        Self {
            code: code.to_string(),
            message: message.into(),
            details: None,
        }
    }

    pub fn with_details(mut self, details: serde_json::Value) -> Self {
        self.details = Some(details);
        self
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

impl From<rusqlite::Error> for IpcError {
    fn from(e: rusqlite::Error) -> Self {
        Self::new("E_SQLITE", e.to_string())
    }
}

impl From<std::io::Error> for IpcError {
    fn from(e: std::io::Error) -> Self {
        Self::new("E_IO", e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serializes_without_details() {
        let err = IpcError::new("E_TEST", "boom");
        let s = serde_json::to_string(&err).unwrap();
        assert_eq!(s, r#"{"code":"E_TEST","message":"boom"}"#);
    }

    #[test]
    fn serializes_with_details() {
        let err = IpcError::new("E_TEST", "boom").with_details(serde_json::json!({ "path": "/x" }));
        let s = serde_json::to_string(&err).unwrap();
        assert!(s.contains(r#""code":"E_TEST""#));
        assert!(s.contains(r#""message":"boom""#));
        assert!(s.contains(r#""details":{"path":"/x"}"#));
    }

    #[test]
    fn from_rusqlite_error_uses_e_sqlite_code() {
        let conn = rusqlite::Connection::open_in_memory().unwrap();
        let sql_err = conn.execute("SELECT * FROM no_such_table", []).unwrap_err();
        let err: IpcError = sql_err.into();
        assert_eq!(err.code, "E_SQLITE");
        assert!(!err.message.is_empty());
    }
}
