//! RetroArch `input_driver` check and repair (`x` breaks Steam Deck controllers).

use std::path::PathBuf;

use async_trait::async_trait;
use serde_json::{Value, json};

use crate::services::ports::RetroArchInputConfig;

/// `retroarch.cfg` locations, relative to the user's home, in lookup order.
const CANDIDATES: [&str; 3] = [
    ".var/app/net.retrodeck.retrodeck/config/retroarch/retroarch.cfg",
    ".var/app/org.libretro.RetroArch/config/retroarch/retroarch.cfg",
    ".config/retroarch/retroarch.cfg",
];

pub struct RetroArchConfigFiles {
    home: PathBuf,
}

impl RetroArchConfigFiles {
    pub fn new(home: impl Into<PathBuf>) -> Self {
        Self { home: home.into() }
    }
}

/// The value of the first `input_driver = …` line, quotes stripped.
fn input_driver_value(text: &str) -> Option<String> {
    text.lines()
        .map(str::trim)
        .filter(|l| l.starts_with("input_driver"))
        .find_map(|line| {
            let (_, value) = line.split_once('=')?;
            Some(value.trim().trim_matches('"').trim_matches('\'').to_string())
        })
}

#[async_trait]
impl RetroArchInputConfig for RetroArchConfigFiles {
    async fn check_input_driver(&self) -> Option<Value> {
        for candidate in CANDIDATES {
            let path = self.home.join(candidate);
            let Ok(text) = tokio::fs::read_to_string(&path).await else {
                continue;
            };
            if let Some(value) = input_driver_value(&text) {
                return Some(json!({
                    "warning": value == "x",
                    "current": value,
                    "config_path": path.to_string_lossy(),
                }));
            }
        }
        None
    }

    async fn fix_input_driver(&self) -> Value {
        let check = self.check_input_driver().await;
        let Some(path) = check
            .filter(|c| c["warning"] == json!(true))
            .and_then(|c| c["config_path"].as_str().map(PathBuf::from))
        else {
            return json!({"success": false, "message": "No fix needed"});
        };
        let result = async {
            let text = tokio::fs::read_to_string(&path).await?;
            let fixed: String = text
                .split_inclusive('\n')
                .map(|line| {
                    if line.trim().starts_with("input_driver") {
                        "input_driver = \"sdl2\"\n".to_string()
                    } else {
                        line.to_string()
                    }
                })
                .collect();
            tokio::fs::write(&path, fixed).await
        };
        match result.await {
            Ok(()) => json!({"success": true, "message": "Changed input_driver to sdl2"}),
            Err(e) => {
                tracing::error!("Failed to fix RetroArch input_driver: {e}");
                json!({"success": false, "message": "Operation failed"})
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    fn write_cfg(home: &std::path::Path, rel: &str, text: &str) -> PathBuf {
        let path = home.join(rel);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    }

    #[tokio::test]
    async fn check_finds_first_candidate_with_the_key() {
        let home = tempfile::tempdir().unwrap();
        let ra = RetroArchConfigFiles::new(home.path());
        assert_eq!(ra.check_input_driver().await, None);
        write_cfg(home.path(), CANDIDATES[0], "video_driver = \"gl\"\n");
        let second = write_cfg(home.path(), CANDIDATES[1], "input_driver = \"x\"\n");
        let r = ra.check_input_driver().await.unwrap();
        assert_eq!(
            r,
            json!({"warning": true, "current": "x", "config_path": second.to_string_lossy()})
        );
        write_cfg(home.path(), CANDIDATES[0], "input_driver = 'sdl2'\n");
        assert_eq!(ra.check_input_driver().await.unwrap()["warning"], false);
        assert_eq!(ra.check_input_driver().await.unwrap()["current"], "sdl2");
    }

    #[tokio::test]
    async fn fix_rewrites_only_the_driver_line() {
        let home = tempfile::tempdir().unwrap();
        let ra = RetroArchConfigFiles::new(home.path());
        assert_eq!(
            ra.fix_input_driver().await,
            json!({"success": false, "message": "No fix needed"})
        );
        let path = write_cfg(home.path(), CANDIDATES[2], "a = 1\ninput_driver = \"x\"\nb = 2\n");
        assert_eq!(
            ra.fix_input_driver().await,
            json!({"success": true, "message": "Changed input_driver to sdl2"})
        );
        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            "a = 1\ninput_driver = \"sdl2\"\nb = 2\n"
        );
        assert_eq!(ra.fix_input_driver().await["message"], "No fix needed");
    }

    #[tokio::test]
    async fn fix_write_failure_reports_operation_failed() {
        let home = tempfile::tempdir().unwrap();
        let path = write_cfg(home.path(), CANDIDATES[2], "input_driver = \"x\"\n");
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o444)).unwrap();
        let r = RetroArchConfigFiles::new(home.path()).fix_input_driver().await;
        assert_eq!(r, json!({"success": false, "message": "Operation failed"}));
    }

    #[test]
    fn value_parsing() {
        assert_eq!(input_driver_value("input_driver=x").as_deref(), Some("x"));
        assert_eq!(
            input_driver_value("input_driver\ninput_driver = \"udev\"").as_deref(),
            Some("udev")
        );
        assert_eq!(input_driver_value("# none"), None);
    }
}
