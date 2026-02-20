//! Integration tests for configuration loading and validation.

use std::path::PathBuf;

use jiezi_cloud_config::{load_from, Environment};

fn test_config_dir() -> PathBuf {
    // Workspace root is three levels up from this source file.
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("config")
}

// Test 1: default config loads without error
#[test]
fn test_load_default_config() {
    let cfg = load_from(test_config_dir()).expect("default config should load");
    assert_eq!(cfg.environment, Environment::Development);
    assert_eq!(cfg.server.port, 8080);
}

// Test 2: server bind_address formats correctly
#[test]
fn test_bind_address() {
    let cfg = load_from(test_config_dir()).unwrap();
    let addr = cfg.server.bind_address();
    assert!(addr.contains(':'), "bind address should contain a colon");
}

// Test 3: auth TTLs are positive
#[test]
fn test_auth_ttls_are_positive() {
    let cfg = load_from(test_config_dir()).unwrap();
    assert!(cfg.auth.access_token_ttl_seconds > 0);
    assert!(cfg.auth.refresh_token_ttl_seconds > 0);
    // Refresh token should outlive access token
    assert!(cfg.auth.refresh_token_ttl_seconds > cfg.auth.access_token_ttl_seconds);
}

// Test 4: validate() passes for the development defaults
#[test]
fn test_validate_development_config() {
    let cfg = load_from(test_config_dir()).unwrap();
    // Development config uses the weak placeholder secret — that is OK.
    assert!(cfg.validate().is_ok());
}

// Test 5: validate() rejects GENERATE sentinel in production mode
#[test]
fn test_validate_rejects_generate_in_production() {
    let mut cfg = load_from(test_config_dir()).unwrap();
    cfg.environment = Environment::Production;
    // "GENERATE" placeholder must be rejected in production.
    assert!(cfg.validate().is_err());
}

// Test 6: validate() rejects non-PEM, non-GENERATE values
#[test]
fn test_validate_rejects_invalid_pem() {
    let mut cfg = load_from(test_config_dir()).unwrap();
    cfg.auth.jwt_private_key_pem = "not-a-pem-and-not-GENERATE".to_owned();
    assert!(cfg.validate().is_err());
}

// Test 7: backup config defaults are sane
#[test]
fn test_backup_config_defaults() {
    let cfg = load_from(test_config_dir()).unwrap();
    assert!(cfg.database.backup.enabled);
    assert!(cfg.database.backup.interval_minutes > 0);
    assert!(cfg.database.backup.keep_count >= 4,
        "keep at least 4 backups (1 hour at 15-min intervals)");
}

// Test 8: Environment::is_production / is_development helpers
#[test]
fn test_environment_helpers() {
    assert!(Environment::Production.is_production());
    assert!(!Environment::Production.is_development());
    assert!(Environment::Development.is_development());
    assert!(!Environment::Development.is_production());
}
