//! Claude Code renderer. Filled in Task 3.
#![allow(dead_code)]

use super::{Harness, HostSnapshot, RenderPlan, Unsupported};
use crate::ipc_error::IpcError;
use crate::service::catalog::model::{Asset, Kind};

pub struct Claude;

impl Harness for Claude {
    fn id(&self) -> &'static str {
        "claude"
    }
    fn render(&self, asset: &Asset) -> Result<RenderPlan, Unsupported> {
        Err(Unsupported {
            harness: "claude",
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
