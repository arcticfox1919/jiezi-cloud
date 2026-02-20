use jiezi_cloud_core::events::DomainEvent;
use jiezi_cloud_core::models::user::Role;
use jiezi_cloud_core::types::{FileId, ShareId, SpaceId, TaskId, UserId};

#[test]
fn test_file_uploaded_event_serde_round_trip() {
    let event = DomainEvent::FileUploaded {
        file_id: FileId::new(),
        user_id: UserId::new(),
        space_id: SpaceId::new(),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    assert!(json.contains("\"type\":\"file_uploaded\""));
    let back: DomainEvent = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(back, DomainEvent::FileUploaded { .. }));
}

#[test]
fn test_user_role_changed_event_serde_round_trip() {
    let event = DomainEvent::UserRoleChanged {
        user_id: UserId::new(),
        old_role: Role::Guest,
        new_role: Role::Member,
    };
    let json = serde_json::to_string(&event).expect("serialize");
    let back: DomainEvent = serde_json::from_str(&json).expect("deserialize");
    if let DomainEvent::UserRoleChanged { new_role, .. } = back {
        assert_eq!(new_role, Role::Member);
    } else {
        panic!("unexpected variant");
    }
}

#[test]
fn test_file_moved_event_with_no_parent_change() {
    let event = DomainEvent::FileMoved {
        file_id: FileId::new(),
        user_id: UserId::new(),
        new_parent_id: None,
        old_name: "report.pdf".into(),
        new_name: "annual-report.pdf".into(),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    let back: DomainEvent = serde_json::from_str(&json).expect("deserialize");
    if let DomainEvent::FileMoved { new_parent_id, new_name, .. } = back {
        assert!(new_parent_id.is_none());
        assert_eq!(new_name, "annual-report.pdf");
    } else {
        panic!("unexpected variant");
    }
}

#[test]
fn test_task_status_changed_event_serde_round_trip() {
    let event = DomainEvent::TaskStatusChanged {
        task_id: TaskId::new(),
        user_id: None,
        kind: "thumbnail_generation".into(),
        status: "completed".into(),
    };
    let json = serde_json::to_string(&event).expect("serialize");
    let back: DomainEvent = serde_json::from_str(&json).expect("deserialize");
    assert!(matches!(back, DomainEvent::TaskStatusChanged { .. }));
}

#[test]
fn test_all_event_variants_serialize_without_panic() {
    let events = vec![
        DomainEvent::FileDeleted { file_id: FileId::new(), user_id: UserId::new() },
        DomainEvent::FilePermanentlyDeleted { file_id: FileId::new(), user_id: UserId::new() },
        DomainEvent::FileRestored { file_id: FileId::new(), user_id: UserId::new() },
        DomainEvent::FileShared {
            file_id: FileId::new(),
            share_id: ShareId::new(),
            created_by: UserId::new(),
        },
        DomainEvent::ShareRevoked { share_id: ShareId::new(), revoked_by: UserId::new() },
        DomainEvent::UserRegistered { user_id: UserId::new() },
        DomainEvent::UserDeactivated { user_id: UserId::new() },
        DomainEvent::SpaceCreated { space_id: SpaceId::new(), owner_id: UserId::new() },
    ];
    for event in events {
        serde_json::to_string(&event).expect("all event variants must serialize");
    }
}
