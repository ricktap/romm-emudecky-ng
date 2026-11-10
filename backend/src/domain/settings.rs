//! The `settings.json` schema: defaults, version, and the pure upgrade steps.
//!
//! Anything that decides what a settings document *should* contain belongs
//! here; reading and writing the file is the persistence adapter's job.

use serde_json::{Map, Value, json};

/// Current `settings.json` schema version. A file stamped higher (written by a
/// newer plugin) is never down-stamped.
pub const SETTINGS_VERSION: i64 = 10;

/// The marker key a corrupt-settings reset leaves for the frontend notice.
pub const RESET_NOTICE_KEY: &str = "_settings_reset_notice";

/// Keys every loaded settings document carries, with their default values.
pub fn default_settings() -> Map<String, Value> {
    let v = json!({
        "romm_url": "",
        "romm_api_token": null,
        "romm_api_token_id": null,
        "romm_api_token_origin": null,
        "romm_api_token_source": null,
        "enabled_platforms": {},
        "enabled_collections": {"user": {}, "smart": {}, "franchise": {}},
        "collection_create_platform_groups": false,
        "preferred_region": "auto",
        "steam_input_mode": "default",
        "switch_firmware_version": "",
        "steamgriddb_api_key": "",
        "romm_allow_insecure_ssl": false,
        "log_level": "warn",
        "save_sync_enabled": false,
        "sync_before_launch": true,
        "sync_after_exit": true,
        "default_slot": "default",
        "autocleanup_limit": 10,
        "device_name": null,
        "platform_cores": {},
    });
    match v {
        Value::Object(map) => map,
        _ => unreachable!("default settings literal is an object"),
    }
}

/// Python-style truthiness of a JSON value (`bool(x)` on the decoded value).
pub fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|f| f != 0.0),
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(o) => !o.is_empty(),
    }
}

/// Truthiness of `map[key]`, `false` when absent.
pub fn key_truthy(map: &Map<String, Value>, key: &str) -> bool {
    map.get(key).is_some_and(truthy)
}

/// The integer `version` of a settings document; anything non-numeric is 0.
pub fn stored_version(map: &Map<String, Value>) -> i64 {
    match map.get("version") {
        Some(Value::Number(n)) => n.as_i64().or_else(|| n.as_f64().map(|f| f as i64)).unwrap_or(0),
        Some(Value::Bool(b)) => i64::from(*b),
        Some(Value::String(s)) => s.trim().parse().unwrap_or(0),
        _ => 0,
    }
}

/// Fill every missing default key and backfill `version: 0` for a pre-versioning file.
pub fn apply_defaults(map: &mut Map<String, Value>) {
    for (key, default) in default_settings() {
        map.entry(key).or_insert(default);
    }
    map.entry("version").or_insert(json!(0));
}

const SAVE_SYNC_KNOBS: [&str; 5] = [
    "save_sync_enabled",
    "sync_before_launch",
    "sync_after_exit",
    "default_slot",
    "autocleanup_limit",
];

/// Fold the legacy save-sync knobs and `device_name` from `save_sync_state.json`.
///
/// Only keys present in the legacy file are copied; device identity stays
/// there. A missing or empty legacy document leaves `settings` unchanged.
pub fn fold_legacy_save_sync_settings(
    settings: &Map<String, Value>,
    legacy: Option<&Map<String, Value>>,
) -> Map<String, Value> {
    let mut folded = settings.clone();
    let Some(legacy) = legacy.filter(|l| !l.is_empty()) else {
        return folded;
    };
    if let Some(Value::Object(knobs)) = legacy.get("settings") {
        for key in SAVE_SYNC_KNOBS {
            if let Some(v) = knobs.get(key) {
                folded.insert(key.into(), v.clone());
            }
        }
    }
    if let Some(name) = legacy.get("device_name") {
        folded.insert("device_name".into(), name.clone());
    }
    folded
}

/// Bring a settings document from any older schema to [`SETTINGS_VERSION`].
/// Value semantics — the input is not mutated.
pub fn migrate_settings(data: &Map<String, Value>) -> Map<String, Value> {
    let mut d = data.clone();
    let version = stored_version(&d);
    if version < 1 {
        if d.remove("disable_steam_input").is_some_and(|v| truthy(&v)) {
            d.insert("steam_input_mode".into(), json!("force_off"));
        }
        if d.remove("debug_logging").is_some_and(|v| truthy(&v)) {
            d.insert("log_level".into(), json!("debug"));
        }
        d.insert("version".into(), json!(1));
    }
    if version < 3 {
        if let Some(Value::Object(flat)) = d.get("enabled_collections") {
            let normalized = normalize_enabled_collections(flat);
            d.insert("enabled_collections".into(), normalized);
        }
        d.insert("version".into(), json!(3));
    }
    if version < 4 {
        d.insert("version".into(), json!(4));
    }
    if version < 5 {
        d.entry("romm_api_token").or_insert(Value::Null);
        d.entry("romm_api_token_id").or_insert(Value::Null);
        d.insert("version".into(), json!(5));
    }
    if version < 6 {
        if key_truthy(&d, "romm_api_token") {
            d.remove("romm_user");
            d.remove("romm_pass");
        }
        d.insert("version".into(), json!(6));
    }
    if version < 7 {
        d.entry("platform_cores").or_insert(json!({}));
        d.insert("version".into(), json!(7));
    }
    if version < 8 {
        d.entry("romm_api_token_origin").or_insert(Value::Null);
        d.insert("version".into(), json!(8));
    }
    if version < 9 {
        let keep_for_legacy_mint = !key_truthy(&d, "romm_api_token") && key_truthy(&d, "romm_pass");
        if !keep_for_legacy_mint {
            d.remove("romm_user");
            d.remove("romm_pass");
        }
        d.insert("version".into(), json!(9));
    }
    if version < 10 {
        let source = if key_truthy(&d, "romm_api_token") {
            json!("minted")
        } else {
            Value::Null
        };
        d.insert("romm_api_token_source".into(), source);
        d.insert("version".into(), json!(10));
    }
    d
}

const BUCKETS: [&str; 3] = ["user", "smart", "franchise"];

fn normalize_enabled_collections(flat: &Map<String, Value>) -> Value {
    let all_dicts = flat.values().all(Value::is_object);
    let keys_are_buckets = flat.keys().all(|k| BUCKETS.contains(&k.as_str()));
    if all_dicts && keys_are_buckets && flat.len() == BUCKETS.len() {
        return Value::Object(flat.clone());
    }
    if all_dicts && keys_are_buckets && !flat.is_empty() {
        let mut out = Map::new();
        for kind in BUCKETS {
            out.insert(kind.into(), flat.get(kind).cloned().unwrap_or_else(|| json!({})));
        }
        return Value::Object(out);
    }
    let mut user = Map::new();
    let mut franchise = Map::new();
    for (key, value) in flat {
        let digits = key.trim_start_matches('-');
        if !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit()) {
            user.insert(key.clone(), json!(truthy(value)));
        } else {
            franchise.insert(key.clone(), json!(truthy(value)));
        }
    }
    json!({"user": user, "smart": {}, "franchise": franchise})
}

#[cfg(test)]
mod tests {
    use super::*;

    fn obj(v: Value) -> Map<String, Value> {
        v.as_object().cloned().unwrap()
    }

    #[test]
    fn v0_deprecated_keys() {
        let out = migrate_settings(&obj(json!({"version": 0, "disable_steam_input": true})));
        assert_eq!(out["steam_input_mode"], "force_off");
        assert!(!out.contains_key("disable_steam_input"));
        let out = migrate_settings(&obj(json!({"version": 0, "disable_steam_input": false})));
        assert!(!out.contains_key("steam_input_mode"));
        assert!(!out.contains_key("disable_steam_input"));
        let out = migrate_settings(&obj(json!({"debug_logging": true, "log_level": "warn"})));
        assert_eq!(out["log_level"], "debug");
        let out = migrate_settings(&obj(json!({"version": 0, "debug_logging": false})));
        assert!(!out.contains_key("log_level"));
        assert_eq!(out["version"], 10);
    }

    #[test]
    fn fresh_and_idempotent_and_pure() {
        let input = obj(json!({}));
        let out = migrate_settings(&input);
        assert_eq!(out["version"], 10);
        assert_eq!(out["romm_api_token"], Value::Null);
        assert_eq!(out["romm_api_token_origin"], Value::Null);
        assert_eq!(out["romm_api_token_source"], Value::Null);
        assert_eq!(out["platform_cores"], json!({}));
        assert!(input.is_empty(), "caller map untouched");
        assert_eq!(migrate_settings(&out), out);
    }

    #[test]
    fn v10_no_change() {
        let input = obj(json!({"version": 10, "romm_api_token": "t", "romm_api_token_source": "user"}));
        assert_eq!(migrate_settings(&input), input);
    }

    #[test]
    fn token_install_drops_credentials_and_stamps_minted() {
        let out = migrate_settings(&obj(json!({
            "version": 4, "romm_api_token": "rmm", "romm_user": "a", "romm_pass": "p"
        })));
        assert!(!out.contains_key("romm_user"));
        assert!(!out.contains_key("romm_pass"));
        assert_eq!(out["romm_api_token_source"], "minted");
        assert_eq!(out["romm_api_token"], "rmm");
    }

    #[test]
    fn tokenless_real_password_survives_for_legacy_mint() {
        let out = migrate_settings(&obj(json!({"version": 4, "romm_user": "a", "romm_pass": "p"})));
        assert_eq!(out["romm_user"], "a");
        assert_eq!(out["romm_pass"], "p");
        assert_eq!(out["romm_api_token_source"], Value::Null);
    }

    #[test]
    fn empty_placeholders_purged() {
        let out = migrate_settings(&obj(json!({"version": 8, "romm_user": "", "romm_pass": ""})));
        assert!(!out.contains_key("romm_user"));
        assert!(!out.contains_key("romm_pass"));
        let out = migrate_settings(&obj(json!({"version": 8, "romm_user": "a"})));
        assert!(!out.contains_key("romm_user"));
    }

    #[test]
    fn existing_slots_not_overwritten() {
        let out = migrate_settings(&obj(json!({
            "version": 6, "platform_cores": {"snes": "x"}, "romm_api_token_origin": "https://h",
            "romm_url": "https://other"
        })));
        assert_eq!(out["platform_cores"], json!({"snes": "x"}));
        assert_eq!(out["romm_api_token_origin"], "https://h");
        let out = migrate_settings(&obj(json!({"version": 7, "romm_url": "https://h"})));
        assert_eq!(
            out["romm_api_token_origin"],
            Value::Null,
            "origin never inferred from url"
        );
    }

    #[test]
    fn collections_split_and_normalize() {
        let out = migrate_settings(&obj(json!({
            "version": 2, "enabled_collections": {"12": true, "-3": 1, "QmFzZQ==": false}
        })));
        assert_eq!(
            out["enabled_collections"],
            json!({"user": {"12": true, "-3": true}, "smart": {}, "franchise": {"QmFzZQ==": false}})
        );
        let out = migrate_settings(&obj(json!({"version": 2, "enabled_collections": {}})));
        assert_eq!(
            out["enabled_collections"],
            json!({"user": {}, "smart": {}, "franchise": {}})
        );
        let nested = json!({"user": {"1": true}, "smart": {"s": true}, "franchise": {}});
        let out = migrate_settings(&obj(json!({"version": 2, "enabled_collections": nested.clone()})));
        assert_eq!(out["enabled_collections"], nested);
        let out = migrate_settings(&obj(
            json!({"version": 2, "enabled_collections": {"user": {"1": true}}}),
        ));
        assert_eq!(
            out["enabled_collections"],
            json!({"user": {"1": true}, "smart": {}, "franchise": {}})
        );
        let out = migrate_settings(&obj(json!({"version": 2})));
        assert!(!out.contains_key("enabled_collections"));
        // A v4+ file is never re-split.
        let weird = json!({"12": true});
        let out = migrate_settings(&obj(json!({"version": 4, "enabled_collections": weird.clone()})));
        assert_eq!(out["enabled_collections"], weird);
    }

    #[test]
    fn fold_legacy() {
        let settings = obj(json!({"save_sync_enabled": false, "device_name": null}));
        let legacy = obj(json!({
            "settings": {"save_sync_enabled": true, "autocleanup_limit": 3},
            "device_name": "Deck", "device_id": "abc"
        }));
        let out = fold_legacy_save_sync_settings(&settings, Some(&legacy));
        assert_eq!(out["save_sync_enabled"], true);
        assert_eq!(out["autocleanup_limit"], 3);
        assert_eq!(out["device_name"], "Deck");
        assert!(!out.contains_key("device_id"));
        assert!(!out.contains_key("sync_after_exit"));
        assert_eq!(settings["save_sync_enabled"], false, "input untouched");
        assert_eq!(fold_legacy_save_sync_settings(&settings, None), settings);
        assert_eq!(fold_legacy_save_sync_settings(&settings, Some(&Map::new())), settings);
        let only_name = obj(json!({"device_name": "X"}));
        assert_eq!(
            fold_legacy_save_sync_settings(&settings, Some(&only_name))["device_name"],
            "X"
        );
    }

    #[test]
    fn helpers() {
        assert!(!truthy(&json!(null)));
        assert!(!truthy(&json!(0)));
        assert!(truthy(&json!(0.5)));
        assert!(!truthy(&json!("")));
        assert!(truthy(&json!("x")));
        assert!(!truthy(&json!([])));
        assert!(truthy(&json!({"a": 1})));
        assert_eq!(stored_version(&obj(json!({"version": "abc"}))), 0);
        assert_eq!(stored_version(&obj(json!({"version": "7"}))), 7);
        assert_eq!(stored_version(&obj(json!({"version": 12}))), 12);
        assert_eq!(stored_version(&obj(json!({"version": null}))), 0);
        assert_eq!(stored_version(&obj(json!({}))), 0);
        let mut m = obj(json!({"log_level": "debug"}));
        apply_defaults(&mut m);
        assert_eq!(m["log_level"], "debug");
        assert_eq!(m["version"], 0);
        assert_eq!(m["platform_cores"], json!({}));
        assert!(!m.contains_key("romm_user"));
    }
}
