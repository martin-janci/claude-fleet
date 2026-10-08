//! Host-clipboard read/write over the same ssh path the rest of the service
//! uses. The script probes for the first available clipboard helper
//! (`wl-copy`/`xclip`/`xsel`/`pbcopy` for set, the `*-paste`/`-o` siblings for
//! get), so a single tool surface works on Wayland, X11, and macOS hosts.

use crate::ipc_error::{codes, IpcError};
use crate::shell::quote;
use crate::ssh::SshClient;
use serde::Deserialize;
use std::sync::Arc;
use std::time::Duration;

/// Cap on bytes pushed through `set_clipboard`. The content travels argv-side
/// in a single `bash -lc` invocation; staying well under typical `ARG_MAX`
/// (≈128 KiB on Linux, 256 KiB on macOS) keeps that safe with room for the
/// surrounding script.
const MAX_CLIPBOARD_BYTES: usize = 64 * 1024;

/// The most the quoted script may come to. Quoting is what grows it: a `'`
/// becomes `'\''` once for the script and again for the ssh argv, so 64 KiB
/// of quotes reached ~1 MiB, past Linux's 128 KiB limit on ONE argument
/// (`MAX_ARG_STRLEN`) — an `E2BIG` from exec, reported as a broken ssh.
const MAX_SCRIPT_ARG_BYTES: usize = 100 * 1024;

const SSH_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, Deserialize, rmcp::schemars::JsonSchema)]
#[schemars(crate = "rmcp::schemars", rename = "HostClipboardParams")]
pub struct GetClipboardArgs {
    /// Host alias whose clipboard to read.
    pub host_alias: String,
}

#[derive(Debug, Deserialize)]
pub struct SetClipboardArgs {
    pub host_alias: String,
    pub content: String,
}

/// Read the host's current clipboard. Returns the raw text (no trailing
/// newline added). `E_CLIPBOARD_UNAVAILABLE` if no helper is installed.
pub async fn get_clipboard(
    args: GetClipboardArgs,
    ssh: &Arc<SshClient>,
) -> Result<String, IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    let script = get_clipboard_script();
    let out = run_bash(&args.host_alias, &script, ssh).await?;
    // Checked first: past the cap `head` closes the pipe, and the helper's
    // SIGPIPE is a failed status that is not the host's fault.
    if out.stdout.len() > MAX_CLIPBOARD_BYTES {
        return Err(too_big_to_read());
    }
    if !out.status.success() {
        return Err(clipboard_err(&out.stderr));
    }
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

/// The read side of the cap `set_clipboard` has always had. The clipboard can
/// hold an image or a large paste (`wl-paste` with no `--type` hands over
/// whatever is there), and all of it was read into memory and answered
/// whole over MCP.
fn too_big_to_read() -> IpcError {
    IpcError::new(
        codes::E_INVALID,
        format!("the host's clipboard holds more than {MAX_CLIPBOARD_BYTES} bytes"),
    )
}

/// Write `content` to the host's clipboard. `E_CLIPBOARD_UNAVAILABLE` if no
/// helper is installed; `E_INVALID` if the payload exceeds the size cap.
pub async fn set_clipboard(args: SetClipboardArgs, ssh: &Arc<SshClient>) -> Result<(), IpcError> {
    crate::validate::host_alias(&args.host_alias)?;
    if args.content.len() > MAX_CLIPBOARD_BYTES {
        return Err(IpcError::new(
            codes::E_INVALID,
            format!(
                "clipboard content {} bytes exceeds cap of {MAX_CLIPBOARD_BYTES} bytes",
                args.content.len()
            ),
        ));
    }
    let script = set_clipboard_script(&args.content);
    if quote(&script).len() > MAX_SCRIPT_ARG_BYTES {
        return Err(IpcError::new(
            codes::E_INVALID,
            "clipboard content is too large once quoted for the shell; \
             it holds many single quotes",
        ));
    }
    let out = run_bash(&args.host_alias, &script, ssh).await?;
    if !out.status.success() {
        return Err(clipboard_err(&out.stderr));
    }
    Ok(())
}

async fn run_bash(
    host_alias: &str,
    script: &str,
    ssh: &Arc<SshClient>,
) -> Result<std::process::Output, IpcError> {
    crate::ssh::run_shell(ssh.as_ref(), host_alias, script, SSH_TIMEOUT).await
}

fn clipboard_err(stderr: &[u8]) -> IpcError {
    let msg = String::from_utf8_lossy(stderr).trim().to_string();
    // The script signals "no helper found" with this exact sentinel so we can
    // map it to a typed code without parsing free text from each clipboard CLI.
    if msg.contains("NO_CLIPBOARD_TOOL") {
        IpcError::new(
            codes::E_CLIPBOARD_UNAVAILABLE,
            "no clipboard tool found on host (install wl-clipboard, xclip, or xsel; \
             macOS has pbcopy/pbpaste built in)",
        )
    } else if msg.is_empty() {
        IpcError::new(
            codes::E_CLIPBOARD,
            "clipboard command failed with no stderr",
        )
    } else {
        IpcError::new(codes::E_CLIPBOARD, msg)
    }
}

/// Probe order matches the precedence we want when several helpers coexist:
/// Wayland first (most modern desktops), then X11 (xclip beats xsel because
/// it's more commonly preinstalled), then macOS. The sentinel `NO_CLIPBOARD_TOOL`
/// is grep-matched in [`clipboard_err`] to produce a typed error code.
///
/// The helper's output goes through `head -c <cap + 1>`, so no more than one
/// byte past the cap ever crosses the wire; `pipefail` keeps a helper that
/// failed (no display) a failure rather than `head`'s success.
fn get_clipboard_script() -> String {
    format!(
        "set -e\n\
         set -o pipefail\n\
         if command -v wl-paste >/dev/null 2>&1; then\n\
           wl-paste --no-newline | head -c {n}\n\
         elif command -v xclip >/dev/null 2>&1; then\n\
           xclip -selection clipboard -o | head -c {n}\n\
         elif command -v xsel >/dev/null 2>&1; then\n\
           xsel --clipboard --output | head -c {n}\n\
         elif command -v pbpaste >/dev/null 2>&1; then\n\
           pbpaste | head -c {n}\n\
         else\n\
           echo NO_CLIPBOARD_TOOL >&2\n\
           exit 127\n\
         fi\n",
        n = MAX_CLIPBOARD_BYTES + 1
    )
}

fn set_clipboard_script(content: &str) -> String {
    format!(
        "set -e\n\
         payload={payload}\n\
         if command -v wl-copy >/dev/null 2>&1; then\n\
           printf %s \"$payload\" | wl-copy\n\
         elif command -v xclip >/dev/null 2>&1; then\n\
           printf %s \"$payload\" | xclip -selection clipboard -i\n\
         elif command -v xsel >/dev/null 2>&1; then\n\
           printf %s \"$payload\" | xsel --clipboard --input\n\
         elif command -v pbcopy >/dev/null 2>&1; then\n\
           printf %s \"$payload\" | pbcopy\n\
         else\n\
           echo NO_CLIPBOARD_TOOL >&2\n\
           exit 127\n\
         fi\n",
        payload = quote(content),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn get_script_probes_each_tool_in_order() {
        let s = get_clipboard_script();
        let wl = s.find("wl-paste").unwrap();
        let xclip = s.find("xclip").unwrap();
        let xsel = s.find("xsel").unwrap();
        let pb = s.find("pbpaste").unwrap();
        assert!(wl < xclip && xclip < xsel && xsel < pb);
        assert!(s.contains("NO_CLIPBOARD_TOOL"));
    }

    #[test]
    fn set_script_embeds_quoted_payload() {
        let s = set_clipboard_script("hi; rm -rf /");
        // The payload must be inert — single-quoted, every metacharacter neutered.
        assert!(s.contains("payload='hi; rm -rf /'"));
        assert!(s.contains("wl-copy"));
        assert!(s.contains("pbcopy"));
    }

    #[test]
    fn set_script_quotes_embedded_single_quote() {
        let s = set_clipboard_script("don't");
        assert!(s.contains(r"payload='don'\''t'"));
    }

    #[tokio::test]
    async fn set_clipboard_rejects_oversize_payload() {
        // Build a payload one byte over the cap and assert E_INVALID without
        // needing an ssh client (size check runs before any IO).
        let oversize = "x".repeat(MAX_CLIPBOARD_BYTES + 1);
        let ssh = Arc::new(SshClient::new());
        let err = set_clipboard(
            SetClipboardArgs {
                host_alias: "local".into(),
                content: oversize,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
    }

    #[test]
    fn the_read_is_capped_on_the_host() {
        let s = get_clipboard_script();
        assert_eq!(
            s.matches(&format!("| head -c {}", MAX_CLIPBOARD_BYTES + 1))
                .count(),
            4,
            "{s}"
        );
        assert!(s.contains("pipefail"));
    }

    #[cfg(unix)]
    #[test]
    fn the_capped_read_hands_over_one_byte_past_the_cap_at_most() {
        // The script's pipeline shape, with `yes` standing in for a helper
        // whose clipboard never ends.
        let script = get_clipboard_script().replace("wl-paste --no-newline", "yes");
        let script = script.replace("command -v wl-paste", "command -v yes");
        let out = std::process::Command::new("bash")
            .args(["-c", &script])
            .output()
            .unwrap();
        assert_eq!(out.stdout.len(), MAX_CLIPBOARD_BYTES + 1);
    }

    #[tokio::test]
    async fn set_clipboard_refuses_a_payload_that_quoting_grows_past_one_argument() {
        // Under the byte cap, but every byte a quote.
        let quotes = "'".repeat(MAX_CLIPBOARD_BYTES);
        let ssh = Arc::new(SshClient::new());
        let err = set_clipboard(
            SetClipboardArgs {
                host_alias: "h1".into(),
                content: quotes,
            },
            &ssh,
        )
        .await
        .unwrap_err();
        assert_eq!(err.code, "E_INVALID");
    }

    #[test]
    fn clipboard_err_maps_missing_tool_sentinel() {
        let e = clipboard_err(b"NO_CLIPBOARD_TOOL\n");
        assert_eq!(e.code, "E_CLIPBOARD_UNAVAILABLE");
        let e2 = clipboard_err(b"xclip: Error: Can't open display");
        assert_eq!(e2.code, "E_CLIPBOARD");
        assert!(e2.message.contains("Can't open display"));
    }
}
