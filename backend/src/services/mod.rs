//! Orchestration over the ports in [`ports`]; services never name an adapter.

pub mod adoption;
pub mod connection;
pub mod downloads;
pub mod launch;
pub mod ports;
pub mod rom_removal;
pub mod settings;

#[cfg(test)]
pub mod testing;
