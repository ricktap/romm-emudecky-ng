//! Orchestration over the ports in [`ports`]; services never name an adapter.

pub mod connection;
pub mod ports;
pub mod settings;

#[cfg(test)]
pub mod testing;
