//! The catalog's argument checks, in one place: the authoring functions
//! ([`super::author`]), the hub's `catalog_admin` ([`super::admin`]) and the
//! desktop's Tauri commands all use these, so a name or a path is refused
//! with the same `E_INVALID` wherever it arrives.

use super::model::is_valid_name;
use super::repo::valid_resource_rel_path;
use super::sync::secrets::is_valid_secret_name;
use crate::ipc_error::codes::E_INVALID;
use crate::ipc_error::IpcError;

/// An asset name: `[a-z0-9][a-z0-9-]*`, so it can never climb out of its
/// kind's folder (`..`, `/`) or hide (`.x`).
pub fn check_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("name '{name}' must match [a-z0-9][a-z0-9-]*"),
        ))
    }
}

/// A layer name: the asset-name rule, since it becomes `layers/<name>.yaml`.
pub fn check_layer_name(name: &str) -> Result<(), IpcError> {
    if is_valid_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("layer name '{name}' must match [a-z0-9][a-z0-9-]*"),
        ))
    }
}

/// A resource path inside an asset: under `resources/`, no empty or `..`
/// segment.
pub fn check_resource_path(rel_path: &str) -> Result<(), IpcError> {
    if valid_resource_rel_path(rel_path) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("invalid resource path: {rel_path}"),
        ))
    }
}

/// A `${NAME}` secret name: `[A-Z0-9_]+`.
pub fn check_secret_name(name: &str) -> Result<(), IpcError> {
    if is_valid_secret_name(name) {
        Ok(())
    } else {
        Err(IpcError::new(
            E_INVALID,
            format!("invalid secret name '{name}'; use [A-Z0-9_]+"),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ipc_error::codes;

    #[test]
    fn hostile_names_and_paths_are_invalid() {
        for bad in ["../x", "a/b", "", ".hidden", "A", "-x"] {
            assert_eq!(check_name(bad).unwrap_err().code, codes::E_INVALID, "{bad}");
            assert_eq!(
                check_layer_name(bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad}"
            );
        }
        for bad in [
            "../x",
            "/abs",
            "resources/../../x",
            "resources/",
            "resources//a",
            "x",
        ] {
            assert_eq!(
                check_resource_path(bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad}"
            );
        }
        for bad in ["bad-name", "", "lower", "A B"] {
            assert_eq!(
                check_secret_name(bad).unwrap_err().code,
                codes::E_INVALID,
                "{bad}"
            );
        }
        check_name("my-skill-2").unwrap();
        check_layer_name("core").unwrap();
        check_resource_path("resources/a/b.sh").unwrap();
        check_secret_name("API_TOKEN_2").unwrap();
    }
}
