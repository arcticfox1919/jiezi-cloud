use jiezi_cloud_core::error::{AppError, AppResult};

#[test]
fn test_not_found_display() {
    let err = AppError::NotFound("users/42".to_owned());
    assert_eq!(err.to_string(), "not found: users/42");
}

#[test]
fn test_unauthorized_display() {
    let err = AppError::Unauthorized("token expired".to_owned());
    assert!(err.to_string().contains("unauthorized"));
}

#[test]
fn test_validation_display() {
    let err = AppError::Validation("username too short".to_owned());
    assert!(err.to_string().contains("validation error"));
}

#[test]
fn test_from_io_error_maps_to_storage_variant() {
    let io_err = std::io::Error::new(std::io::ErrorKind::PermissionDenied, "access denied");
    let app_err: AppError = io_err.into();
    assert!(matches!(app_err, AppError::Storage(_)));
}

#[test]
fn test_from_json_error_maps_to_serialization_variant() {
    let json_err = serde_json::from_str::<String>("not valid json!!!").unwrap_err();
    let app_err: AppError = json_err.into();
    assert!(matches!(app_err, AppError::Serialization(_)));
}

#[test]
fn test_app_result_ok_carries_value() {
    let result: AppResult<u32> = Ok(99);
    assert!(matches!(result, Ok(99)));
}

#[test]
fn test_app_result_err_is_accessible() {
    let result: AppResult<u32> = Err(AppError::Forbidden("read-only space".to_owned()));
    assert!(result.is_err());
    let Err(err) = result else {
        panic!("expected Err variant");
    };
    assert!(err.to_string().contains("forbidden"));
}
