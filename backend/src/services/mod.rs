//! Orchestration over the ports in [`ports`]; services never name an adapter.

pub mod adoption;
pub mod connection;
pub mod downloads;
pub mod launch;
pub mod launch_choices;
pub mod library;
pub mod library_state;
pub mod ports;
pub mod rom_removal;
pub mod settings;

#[cfg(test)]
pub mod testing;
pub mod library_fetcher;
pub mod sync_reporter;
pub mod sync_orchestrator;
pub mod artwork;
pub mod shortcut_removal;
pub mod game_detail;
pub mod game_covers;
pub mod steamgrid;

#[cfg(test)]
mod game_artwork_tests;
