//! SeaORM entity definitions for the authentication module.
//!
//! Each sub-module maps to a database table.  Entities are used strictly for
//! persistence; they are converted to/from the domain types defined in
//! `jiezi-cloud-core` at the repository boundary.

pub mod refresh_tokens;
pub mod users;
