//! The plugin's own metadata (`package.json`), read once at startup.

use std::path::Path;

/// The `version` field of `<plugin_dir>/package.json`, or `0.0.0` when the
/// file is missing or malformed (startup must not fail on it).
pub async fn read_plugin_version(plugin_dir: &Path) -> String {
    let Ok(bytes) = tokio::fs::read(plugin_dir.join("package.json")).await else {
        return "0.0.0".into();
    };
    serde_json::from_slice::<serde_json::Value>(&bytes)
        .ok()
        .and_then(|v| v.get("version").and_then(|s| s.as_str()).map(str::to_string))
        .unwrap_or_else(|| "0.0.0".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reads_version_or_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_plugin_version(dir.path()).await, "0.0.0");
        std::fs::write(dir.path().join("package.json"), "{oops").unwrap();
        assert_eq!(read_plugin_version(dir.path()).await, "0.0.0");
        std::fs::write(dir.path().join("package.json"), r#"{"version": 3}"#).unwrap();
        assert_eq!(read_plugin_version(dir.path()).await, "0.0.0");
        std::fs::write(dir.path().join("package.json"), r#"{"name": "x", "version": "0.40.0"}"#).unwrap();
        assert_eq!(read_plugin_version(dir.path()).await, "0.40.0");
    }
}
