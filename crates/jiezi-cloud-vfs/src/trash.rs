//! Trash (soft-delete) helpers for the virtual file system.
//!
//! All soft-delete, restore and permanent-delete operations are implemented
//! in [`crate::repository::FileNodeRepository`].  This module is reserved
//! for future extensions such as:
//!
//! - Scheduled expiry of items that have been in the trash longer than a
//!   configurable retention period.
//! - Aggregate accounting for the space consumed by trashed files.

