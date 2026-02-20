use chrono::Utc;
use jiezi_cloud_core::models::space::{Space, SpaceMember};
use jiezi_cloud_core::models::user::Role;
use jiezi_cloud_core::types::{FileId, SpaceId, UserId};

fn make_space(quota: Option<u64>, used: u64) -> Space {
    Space {
        id: SpaceId::new(),
        name: "My Files".into(),
        description: None,
        owner_id: UserId::new(),
        root_id: FileId::new(),
        storage_quota: quota,
        storage_used: used,
        created_at: Utc::now(),
        updated_at: Utc::now(),
    }
}

#[test]
fn test_space_quota_not_exceeded_when_under_limit() {
    let space = make_space(Some(100), 50);
    assert!(!space.is_quota_exceeded());
}

#[test]
fn test_space_quota_exceeded_when_over_limit() {
    let space = make_space(Some(100), 150);
    assert!(space.is_quota_exceeded());
}

#[test]
fn test_space_quota_never_exceeded_when_unlimited() {
    let space = make_space(None, u64::MAX);
    assert!(!space.is_quota_exceeded());
}

#[test]
fn test_space_remaining_quota_calculated_correctly() {
    let space = make_space(Some(100), 40);
    assert_eq!(space.remaining_quota(), Some(60));
}

#[test]
fn test_space_remaining_quota_saturates_at_zero() {
    let space = make_space(Some(100), 200);
    assert_eq!(space.remaining_quota(), Some(0));
}

#[test]
fn test_space_serde_round_trip() {
    let space = make_space(Some(1_073_741_824), 512_000);
    let json = serde_json::to_string(&space).expect("serialize");
    let back: Space = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(space.id, back.id);
    assert_eq!(space.name, back.name);
}

#[test]
fn test_space_member_serde_round_trip() {
    let member = SpaceMember {
        space_id: SpaceId::new(),
        user_id: UserId::new(),
        role: Role::Member,
        joined_at: Utc::now(),
    };
    let json = serde_json::to_string(&member).expect("serialize");
    let back: SpaceMember = serde_json::from_str(&json).expect("deserialize");
    assert_eq!(member.role, back.role);
}
