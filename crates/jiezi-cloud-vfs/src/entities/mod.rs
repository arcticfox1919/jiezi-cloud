//! SeaORM entity definitions for the VFS module.
//!
//! Each sub-module maps to a database table.  Entities are used strictly for
//! persistence; all other code works with the domain types defined in
//! `jiezi-cloud-core`.

pub mod file_node_paths;
pub mod file_nodes;
pub mod spaces;
