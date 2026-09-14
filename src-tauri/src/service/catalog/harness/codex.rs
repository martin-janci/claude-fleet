//! Codex renderer. Filled in Task 4.
#![allow(dead_code)]

use super::{Harness, HostSnapshot, RenderPlan, Unsupported};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, Kind};

pub struct Codex;

impl Harness for Codex {
    fn id(&self) -> &'static str {
        "codex"
    }
    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        Err(Unsupported {
            harness: "codex",
            kind: asset.kind(),
        })
    }
    fn scan_script(&self) -> Option<String> {
        None
    }
    fn parse_scan(&self, _stdout: &str) -> Result<HostSnapshot, IpcError> {
        Ok(HostSnapshot::default())
    }
    fn installed(&self, _snap: &HostSnapshot) -> Vec<(Kind, String)> {
        vec![]
    }
}
