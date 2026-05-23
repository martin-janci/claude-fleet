//! Generates the control-API tool reference from the live MCP tool router.
//!
//! The committed `docs/control-api-reference.md` must equal `render_reference()`;
//! the `reference_is_current` test enforces it (and so does CI via `cargo test`).
//! Regenerate after changing any tool with:
//!   REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current
//!
//! Entirely `#[cfg(test)]`: it touches no production code path and adds no
//! public API surface.

use crate::mcp::FleetTools;

const HEADER: &str = "<!-- GENERATED FILE — do not edit by hand.\n     \
Regenerate with: REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current -->\n";

/// Render the full reference markdown.
pub(crate) fn render_reference() -> String {
    let mut out = String::new();
    out.push_str(HEADER);
    out.push_str("\n# claude-fleet Control API — Tool Reference\n\n");
    out.push_str(
        "Auto-generated from the embedded MCP tool router. \
See [`control-api.md`](control-api.md) for the narrative guide.\n\n",
    );

    // --- MCP tools ---
    out.push_str("## MCP tools\n\n");
    let mut tools = FleetTools::tool_router_for_doc().list_all();
    tools.sort_by(|a, b| a.name.cmp(&b.name));
    for t in &tools {
        out.push_str(&format!("### `{}`\n\n", t.name));
        if let Some(d) = &t.description {
            out.push_str(d.trim());
            out.push_str("\n\n");
        }
        if let Some(props) = t.input_schema.get("properties").and_then(|v| v.as_object()) {
            if !props.is_empty() {
                let mut names: Vec<&String> = props.keys().collect();
                names.sort();
                let rendered: Vec<String> = names.iter().map(|n| format!("`{n}`")).collect();
                out.push_str(&format!("Parameters: {}\n\n", rendered.join(", ")));
            }
        }
    }

    // --- Tauri commands ---
    out.push_str("## Tauri IPC commands\n\n");
    out.push_str("Frontend commands registered in `src/lib.rs`:\n\n");
    for cmd in tauri_commands() {
        out.push_str(&format!("- `{cmd}`\n"));
    }
    out.push('\n');
    out
}

/// Extract command identifiers from the `generate_handler![ … ]` block in lib.rs.
fn tauri_commands() -> Vec<String> {
    let src = include_str!("../lib.rs");
    let start = src
        .find("generate_handler![")
        .expect("generate_handler! macro present in lib.rs");
    let rest = &src[start + "generate_handler![".len()..];
    let end = rest.find(']').expect("closing ] of generate_handler!");
    rest[..end]
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty() && !s.starts_with("//"))
        .map(str::to_string)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn doc_path() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../docs/control-api-reference.md")
    }

    #[test]
    fn extracts_some_commands() {
        let cmds = tauri_commands();
        assert!(cmds.contains(&"commands::sessions::list_sessions".to_string()));
        assert!(
            cmds.len() > 20,
            "expected many commands, got {}",
            cmds.len()
        );
    }

    #[test]
    fn renders_known_tool() {
        let md = render_reference();
        assert!(
            md.contains("### `list_sessions`"),
            "list_sessions tool missing"
        );
    }

    #[test]
    fn reference_is_current() {
        let expected = render_reference();
        let path = doc_path();
        if std::env::var("REGEN_DOCS").is_ok() {
            std::fs::write(&path, &expected).expect("write control-api-reference.md");
            return;
        }
        let actual = std::fs::read_to_string(&path).unwrap_or_default();
        assert_eq!(
            actual, expected,
            "\n\ndocs/control-api-reference.md is stale. Regenerate with:\n  \
REGEN_DOCS=1 cargo test --manifest-path src-tauri/Cargo.toml reference_is_current\n"
        );
    }
}
