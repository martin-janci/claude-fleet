//! HTTP/1.1 response-head parsing and chunked-transfer decoding for the two
//! hand-rolled clients in this module tree: [`super::remote`]'s one-shot
//! `POST /mcp` and [`super::events`]'s long-lived `GET /events`.
//!
//! The implementation (and its tests) moved to `fleet_core::net::http1` with
//! work graph M3.0, where fleet-core's tracker client shares it; this module
//! keeps the names the two callers already import.

pub(crate) use fleet_core::net::http1::{dechunk, find, head_is_chunked, parse_status, Dechunker};
