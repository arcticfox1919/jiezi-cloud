use chrono::{DateTime, Duration, Utc};
use jiezi_cloud_core::models::share::{ShareAccess, ShareLink};
use jiezi_cloud_core::types::{FileId, ShareId, UserId};

fn make_link(
    expires_at: Option<DateTime<Utc>>,
    download_limit: Option<u32>,
    download_count: u32,
) -> ShareLink {
    ShareLink {
        id: ShareId::new(),
        file_id: FileId::new(),
        created_by: UserId::new(),
        access: ShareAccess::ReadOnly,
        password_hash: None,
        expires_at,
        download_limit,
        download_count,
        created_at: Utc::now(),
    }
}

#[test]
fn test_valid_link_with_no_constraints() {
    let link = make_link(None, None, 0);
    assert!(link.is_valid_at(&Utc::now()));
}

#[test]
fn test_expired_link_is_invalid() {
    let expired_at = Utc::now() - Duration::hours(1);
    let link = make_link(Some(expired_at), None, 0);
    assert!(!link.is_valid_at(&Utc::now()));
}

#[test]
fn test_future_expiry_link_is_valid() {
    let expires_at = Utc::now() + Duration::hours(24);
    let link = make_link(Some(expires_at), None, 0);
    assert!(link.is_valid_at(&Utc::now()));
}

#[test]
fn test_download_limit_exhausted_invalidates_link() {
    let link = make_link(None, Some(5), 5);
    assert!(!link.is_valid_at(&Utc::now()));
}

#[test]
fn test_download_limit_not_exhausted_keeps_link_valid() {
    let link = make_link(None, Some(5), 4);
    assert!(link.is_valid_at(&Utc::now()));
}

#[test]
fn test_password_protection_detection() {
    let mut link = make_link(None, None, 0);
    assert!(!link.is_password_protected());
    link.password_hash = Some("$argon2id$...".into());
    assert!(link.is_password_protected());
}

#[test]
fn test_share_link_password_hash_not_serialized() {
    let mut link = make_link(None, None, 0);
    link.password_hash = Some("secret_hash".into());
    let json = serde_json::to_string(&link).expect("serialize");
    assert!(!json.contains("secret_hash"), "password_hash must be skipped in JSON output");
}

#[test]
fn test_share_link_serde_round_trip() {
    let link = make_link(None, Some(10), 3);
    let json = serde_json::to_string(&link).expect("serialize");
    let back: ShareLink = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(link.id, back.id);
    assert_eq!(link.download_limit, back.download_limit);
    assert_eq!(link.download_count, back.download_count);
}
