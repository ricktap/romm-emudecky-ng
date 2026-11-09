//! Per-platform sync-enabled defaulting.
//!
//! The empty `enabled_platforms` map means "all enabled". Once a map is
//! persisted it is read literally, so absent ids are disabled.

use serde_json::{Map, Value};
use crate::domain::settings::truthy;

pub fn resolve_sync_enabled(stored: &Map<String, Value>, pid: &str) -> bool {
    stored.get(pid).map(truthy).unwrap_or(stored.is_empty())
}

pub fn materialize_enabled_platforms(
    stored: &Map<String, Value>,
    platform_ids: &[String],
) -> Option<Map<String, Value>> {
    if !stored.is_empty() || platform_ids.is_empty() {
        return None;
    }
    Some(
        platform_ids
            .iter()
            .map(|pid| (pid.clone(), Value::Bool(true)))
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn empty_map_means_enabled_until_materialized() {
        let empty = Map::new();
        assert!(resolve_sync_enabled(&empty, "1"));
        let full = materialize_enabled_platforms(&empty, &["1".into(), "2".into()]).unwrap();
        assert_eq!(full["1"], json!(true));
        assert_eq!(full["2"], json!(true));
    }

    #[test]
    fn explicit_map_is_read_literally() {
        let mut stored = Map::new();
        stored.insert("1".into(), json!(false));
        assert!(!resolve_sync_enabled(&stored, "1"));
        assert!(!resolve_sync_enabled(&stored, "2"));
        assert!(materialize_enabled_platforms(&stored, &["1".into()]).is_none());
    }

    #[test]
    fn no_platforms_leave_the_empty_map_sentinel_in_place() {
        assert!(materialize_enabled_platforms(&Map::new(), &[]).is_none());
    }

    #[test]
    fn legacy_truthy_preferences_match_python_filtering() {
        let stored = json!({"1": 1, "2": "yes", "3": 0, "4": null});
        let stored = stored.as_object().unwrap();
        assert!(resolve_sync_enabled(stored, "1"));
        assert!(resolve_sync_enabled(stored, "2"));
        assert!(!resolve_sync_enabled(stored, "3"));
        assert!(!resolve_sync_enabled(stored, "4"));
        assert!(!resolve_sync_enabled(stored, "5"));
    }
}
