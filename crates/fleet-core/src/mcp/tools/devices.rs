//! MCP tool: debug devices (`service::debug_devices`, `docs/debug-devices.md`).
//! One tool by `action`: a host's Claude lists, claims and drives the phones,
//! emulators and simulators it may use, wherever they are plugged in; a
//! person also labels, shares and forgets them.

use super::*;
use crate::ipc_error::lock;
use crate::service::debug_devices::{self as devices, Asker, InstallFrom};
use crate::ssh::SshExec;

/// Who a call speaks for: the caller's scope, and a claim's holder. A
/// host's session that proves its pane holds claims as itself
/// (`host:<alias>#<session>`), so two sessions on one host do not share one.
fn asker(store: &Mutex<Store>, caller: &Caller) -> Result<Asker, McpError> {
    let s = lock(store).map_err(to_mcp_err)?;
    let scope = caller.view_scope(&s).map_err(to_mcp_err)?;
    let holder = match scope.proven_session {
        Some(id) if caller.host_alias.is_some() => format!("{}#{id}", caller.label()),
        _ => caller.label(),
    };
    Ok(Asker { scope, holder })
}

fn need<'a>(v: &'a Option<String>, what: &str, action: &str) -> Result<&'a str, McpError> {
    v.as_deref()
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| mcp_err(codes::E_INVALID, format!("{action} needs {what}"), None))
}

#[tool_router(router = devices_router, vis = "pub(super)")]
impl FleetTools {
    #[tool(description = "Phones, emulators and simulators on any fleet \
        host, used from any session that may see them; commands run on the \
        device's host. run: one adb / simctl / devicectl command. install \
        copies the app from your host. screenshot answers an image. A claim \
        keeps others off (E_CONFLICT); use extends it.")]
    pub(super) async fn debug_devices(
        &self,
        Extension(caller): Extension<Caller>,
        Parameters(p): Parameters<DebugDevicesParams>,
    ) -> Result<CallToolResult, McpError> {
        audit(
            "debug_devices",
            &format!(
                "action={} device={}",
                p.action.escape_debug(),
                p.device.as_deref().unwrap_or("").escape_debug()
            ),
        );
        let who = asker(&self.store, &caller)?;
        let ssh: Arc<dyn SshExec> = self.ssh.clone();
        let action = p.action.as_str();
        match action {
            "list" => {
                let out = devices::list(&self.store, ssh, &who, p.refresh.unwrap_or(false))
                    .await
                    .map_err(to_mcp_err)?;
                ok_json_compact(&out)
            }
            "scan" => {
                let out = devices::scan(&self.store, &*ssh, &who, p.host.as_deref())
                    .await
                    .map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "hosts": out }))
            }
            "claim" => {
                let d = devices::claim(
                    &self.store,
                    &who,
                    need(&p.device, "device", action)?,
                    p.claim_s,
                    p.note.as_deref(),
                )
                .map_err(to_mcp_err)?;
                ok_json_compact(&d)
            }
            "release" => {
                let d = devices::release(&self.store, &who, need(&p.device, "device", action)?)
                    .map_err(to_mcp_err)?;
                ok_json_compact(&d)
            }
            "run" => {
                let args = p.args.clone().unwrap_or_default();
                let out = devices::run(
                    &self.store,
                    &*ssh,
                    &who,
                    need(&p.device, "device", action)?,
                    &args,
                    p.timeout_s,
                )
                .await
                .map_err(to_mcp_err)?;
                ok_json_compact(&out)
            }
            "install" => {
                let device = need(&p.device, "device", action)?;
                let path = need(&p.path, "path", action)?;
                // A host's session installs from its own host; a person names
                // the host, or means the device's own.
                let from_host = match (caller.host_alias.as_deref(), p.host.as_deref()) {
                    (Some(own), _) => own.to_string(),
                    (None, Some(h)) => h.to_string(),
                    (None, None) => {
                        let s = lock(&self.store).map_err(to_mcp_err)?;
                        devices::resolve(&s, &who, device).map_err(to_mcp_err)?.host
                    }
                };
                let out = devices::install(
                    &self.store,
                    &*ssh,
                    &who,
                    device,
                    InstallFrom {
                        host: &from_host,
                        path,
                    },
                    p.downgrade.unwrap_or(false),
                )
                .await
                .map_err(to_mcp_err)?;
                ok_json_compact(&out)
            }
            "logs" => {
                let out = devices::logs(
                    &self.store,
                    &*ssh,
                    &who,
                    need(&p.device, "device", action)?,
                    p.lines,
                    p.since_s,
                    p.filter.as_deref(),
                    p.contains.as_deref(),
                )
                .await
                .map_err(to_mcp_err)?;
                ok_json_compact(&out)
            }
            "screenshot" => {
                use base64::Engine as _;
                let (d, bytes, mime) = devices::screenshot(
                    &self.store,
                    &*ssh,
                    &who,
                    need(&p.device, "device", action)?,
                )
                .await
                .map_err(to_mcp_err)?;
                let data = base64::engine::general_purpose::STANDARD.encode(&bytes);
                Ok(CallToolResult::success(vec![
                    Content::text(format!("{} on {}: {} bytes", d.title, d.host, bytes.len())),
                    Content::image(data, mime),
                ]))
            }
            "boot" => {
                let said = devices::boot(
                    &self.store,
                    &*ssh,
                    &who,
                    p.device.as_deref(),
                    p.host.as_deref(),
                    p.name.as_deref(),
                )
                .await
                .map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "state": said }))
            }
            "shutdown" => {
                let d =
                    devices::shutdown(&self.store, &*ssh, &who, need(&p.device, "device", action)?)
                        .await
                        .map_err(to_mcp_err)?;
                ok_json_compact(&d)
            }
            "configure" => {
                let device = need(&p.device, "device", action)?;
                let id = {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    devices::resolve(&s, &who, device).map_err(to_mcp_err)?.id
                };
                let d = devices::configure(&self.store, &who, id, p.label.as_deref(), p.shared)
                    .map_err(to_mcp_err)?;
                ok_json_compact(&d)
            }
            "forget" => {
                let device = need(&p.device, "device", action)?;
                let id = {
                    let s = lock(&self.store).map_err(to_mcp_err)?;
                    devices::resolve(&s, &who, device).map_err(to_mcp_err)?.id
                };
                let removed = devices::forget(&self.store, &who, id).map_err(to_mcp_err)?;
                ok_json_compact(&serde_json::json!({ "removed": removed }))
            }
            other => Err(mcp_err(
                codes::E_INVALID,
                format!(
                    "action must be list | scan | claim | release | run | install | logs | \
                     screenshot | boot | shutdown | configure | forget, got {other:?}"
                ),
                None,
            )),
        }
    }
}
