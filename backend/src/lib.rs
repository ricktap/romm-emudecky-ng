//! Native backend of the EmuDecky RomM Manager Decky plugin.
//!
//! Layering mirrors the Python backend it replaces: `domain` is pure compute,
//! `services` orchestrate through the traits in `services::ports`, `adapters`
//! own all I/O, `rpc` speaks the shim protocol, and `app` is the composition
//! root. `main.rs` is the process entry point.

pub mod adapters;
pub mod app;
pub mod domain;
pub mod rpc;
pub mod services;
