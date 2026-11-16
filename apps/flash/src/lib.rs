//! The `flash` binary's logic that does not need a terminal: request
//! execution (files and the clipboard) and latency summaries. The
//! renderer and input conversion live in `flash-term`; the main loop in
//! `main.rs`.

#![forbid(unsafe_code)]

pub mod latency;
pub mod requests;
