use chrono::Utc;
use jiezi_cloud_core::models::user::{RegisterRequest, Role, TokenPair, User};
use jiezi_cloud_core::types::UserId;

#[test]
fn test_role_display() {
    assert_eq!(Role::Owner.to_string(), "owner");
    assert_eq!(Role::Admin.to_string(), "admin");
    assert_eq!(Role::Member.to_string(), "member");
    assert_eq!(Role::Guest.to_string(), "guest");
}

#[test]
fn test_role_serde_round_trip() {
    for role in [Role::Owner, Role::Admin, Role::Member, Role::Guest] {
        let json = serde_json::to_string(&role).expect("serialize role");
        let back: Role = serde_json::from_str(&json).expect("deserialize role");
        assert_eq!(role, back);
    }
}

#[test]
fn test_token_pair_serde_round_trip() {
    let pair = TokenPair {
        access_token: "aaa.bbb.ccc".into(),
        refresh_token: "xxx.yyy.zzz".into(),
        expires_in: 900,
    };
    let json = serde_json::to_string(&pair).expect("serialize");
    let back: TokenPair = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(pair.access_token, back.access_token);
    assert_eq!(pair.refresh_token, back.refresh_token);
    assert_eq!(pair.expires_in, back.expires_in);
}

#[test]
fn test_user_password_hash_not_serialized() {
    let user = User {
        id: UserId::new(),
        username: "alice".into(),
        email: "alice@example.com".into(),
        password_hash: "secret_hash".into(),
        role: Role::Member,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        is_active: true,
        display_name: None,
        avatar_url: None,
        storage_quota: None,
        storage_used: 0,
        failed_login_count: 0,
        locked_until: None,
        last_failed_login_at: None,
        email_verified: false,
    };
    let json = serde_json::to_string(&user).expect("serialize user");
    assert!(
        !json.contains("secret_hash"),
        "password_hash must never appear in serialised output"
    );
}

#[test]
fn test_register_request_serde_round_trip() {
    let req = RegisterRequest {
        username: "bob".into(),
        email: "bob@example.com".into(),
        password: "hunter2".into(),
        display_name: Some("Bob Smith".into()),
        email_otp: None,
    };
    let json = serde_json::to_string(&req).expect("serialize");
    let back: RegisterRequest = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(req.username, back.username);
    assert_eq!(req.email, back.email);
}
