//! HTTP/1.1 response-head parsing and chunked-transfer decoding for the two
//! hand-rolled hub clients: [`super`]'s one-shot `POST /mcp` and the
//! desktop's (`src-tauri` `backend::events`) long-lived `GET /events`.
//!
//! The implementation (and its tests) is [`crate::net::http1`]'s, shared
//! with the tracker client since work graph M3.0; this module keeps the names
//! the hub clients already import.

pub use crate::net::http1::{dechunk, find, head_is_chunked, parse_status, Dechunker};
