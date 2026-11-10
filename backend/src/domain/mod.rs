//! Pure compute: no I/O, no state, no service or adapter imports.

pub mod archive_transfer;
pub mod disc_selection;
pub mod download_failure;
pub mod download_job;
pub mod emulator_table;
pub mod errors;
pub mod install_layout;
pub mod launch;
pub mod platform_prefs;
pub mod rom_files;
pub mod rom_install;
pub mod settings;
pub mod sync_run;
pub mod title_match;
pub mod url_host;
pub mod version;
pub mod work_unit;
pub mod sibling_group;
pub mod sibling_resolution;
pub mod sync_diff;
pub mod shortcut_data;
pub mod steam_input;
pub mod switch_firmware;
pub mod sgdb_artwork;

pub mod iso_time;
pub mod playtime;
pub mod self_update;
pub mod version_picker;
