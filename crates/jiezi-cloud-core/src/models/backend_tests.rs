//! Unit tests for [`super::BackendDriver`], config structs, and
//! [`super::ReplicationPolicy`].
//!
//! This file is pulled in by `backend.rs` via:
//!
//! ```rust,ignore
//! #[cfg(test)]
//! #[path = "backend_tests.rs"]
//! mod tests;
//! ```
//!
//! Keeping tests in a separate file avoids mixing production code with test
//! scaffolding and makes `backend.rs` easy to read at a glance.

use super::{
    BackendDriver, LocalFsConfig, ReplicationPolicy, S3Config, WebDavConfig,
    DRIVER_LOCAL_FS, DRIVER_S3_COMPATIBLE, DRIVER_WEBDAV,
};
use crate::types::BackendId;

// ─── BackendDriver round-trips ────────────────────────────────────────────────

#[test]
fn driver_local_fs_round_trip() {
    let cfg = LocalFsConfig { root_dir: "/data/chunks".into() };
    let driver = BackendDriver::local_fs(cfg.clone());

    // JSON must contain the "driver" discriminant
    let json = serde_json::to_string(&driver).unwrap();
    assert!(json.contains(r#""driver":"local_fs""#), "json={json}");
    // "type" key from the old enum must not appear
    assert!(!json.contains(r#""type""#));

    // Deserialise back and check typed accessor
    let round: BackendDriver = serde_json::from_str(&json).unwrap();
    assert_eq!(round.kind, DRIVER_LOCAL_FS);
    assert_eq!(round.as_local_fs(), Some(cfg));
}

#[test]
fn driver_s3_round_trip() {
    let cfg = S3Config {
        endpoint: Some("https://minio.internal:9000".into()),
        bucket: "my-bucket".into(),
        region: "us-east-1".into(),
        prefix: None,
        access_key_id: "AKIAIOSFODNN7EXAMPLE".into(),
        secret_access_key: "wJalrXUtnFEMI/K7MDENG".into(),
    };
    let driver = BackendDriver::s3_compatible(cfg.clone());

    let json = serde_json::to_string(&driver).unwrap();
    assert!(json.contains(r#""driver":"s3_compatible""#), "json={json}");
    // prefix is None → should be omitted
    assert!(!json.contains(r#""prefix""#));

    let round: BackendDriver = serde_json::from_str(&json).unwrap();
    assert_eq!(round.kind, DRIVER_S3_COMPATIBLE);
    assert_eq!(round.as_s3_compatible(), Some(cfg));
}

#[test]
fn driver_webdav_round_trip() {
    let cfg = WebDavConfig {
        url: "https://nextcloud.example.com/remote.php/dav/files/alice/".into(),
        username: Some("alice".into()),
        password: Some("hunter2".into()),
        bearer_token: None,
    };
    let driver = BackendDriver::webdav(cfg.clone());

    let json = serde_json::to_string(&driver).unwrap();
    assert!(json.contains(r#""driver":"webdav""#), "json={json}");

    let round: BackendDriver = serde_json::from_str(&json).unwrap();
    assert_eq!(round.kind, DRIVER_WEBDAV);
    assert_eq!(round.as_webdav(), Some(cfg));
}

#[test]
fn driver_custom_preserves_unknown_fields() {
    let mut map = serde_json::Map::new();
    map.insert("token".into(), serde_json::Value::String("abc123".into()));
    map.insert("root_path".into(), serde_json::Value::String("/备份".into()));

    let driver = BackendDriver::custom("aliyun_pan", map.clone());
    let json = serde_json::to_string(&driver).unwrap();
    assert!(json.contains(r#""driver":"aliyun_pan""#), "json={json}");
    assert!(json.contains("abc123"));

    let round: BackendDriver = serde_json::from_str(&json).unwrap();
    assert_eq!(round.kind, "aliyun_pan");
    assert_eq!(round.config, map);
}

#[test]
fn accessor_returns_none_for_wrong_kind() {
    let driver = BackendDriver::local_fs(LocalFsConfig { root_dir: "/".into() });
    // Wrong kind → typed accessors return None, not panic
    assert_eq!(driver.as_s3_compatible(), None);
    assert_eq!(driver.as_webdav(), None);
}

#[test]
fn is_builtin_true_for_known_drivers() {
    assert!(BackendDriver::local_fs(LocalFsConfig { root_dir: "/".into() }).is_builtin());
    assert!(BackendDriver::s3_compatible(S3Config {
        endpoint: None,
        bucket: "b".into(),
        region: "us-east-1".into(),
        prefix: None,
        access_key_id: "k".into(),
        secret_access_key: "s".into(),
    }).is_builtin());
}

#[test]
fn is_builtin_false_for_custom_driver() {
    assert!(!BackendDriver::custom("my_custom_driver", Default::default()).is_builtin());
}

#[test]
fn display_name_passthrough_for_unimplemented_drivers() {
    // Only the 3 built-in drivers have translated display names.
    // Any other driver kind (including well-known ones not yet implemented)
    // is returned as-is — the caller can localise it in the UI layer.
    let aliyun = BackendDriver::custom("aliyun_pan", Default::default());
    assert_eq!(aliyun.display_name(), "aliyun_pan");
    let baidu = BackendDriver::custom("baidu_pan", Default::default());
    assert_eq!(baidu.display_name(), "baidu_pan");
}

#[test]
fn display_name_passthrough_for_unknown_driver() {
    let d = BackendDriver::custom("my_plugin", Default::default());
    assert_eq!(d.display_name(), "my_plugin");
}

// ─── ReplicationPolicy ────────────────────────────────────────────────────────

#[test]
fn replication_policy_default_is_all() {
    assert_eq!(ReplicationPolicy::default(), ReplicationPolicy::All);
}

#[test]
fn replication_policy_min_n_round_trip() {
    let p = ReplicationPolicy::MinN { n: 2 };
    let json = serde_json::to_string(&p).unwrap();
    let round: ReplicationPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(round, p);
}

#[test]
fn replication_policy_specific_round_trip() {
    let p = ReplicationPolicy::Specific {
        backend_ids: vec![BackendId::new("primary"), BackendId::new("backup")],
    };
    let json = serde_json::to_string(&p).unwrap();
    let round: ReplicationPolicy = serde_json::from_str(&json).unwrap();
    assert_eq!(round, p);
}
