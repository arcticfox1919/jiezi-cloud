//! SeaORM entity for the `file_node_paths` closure table.
//!
//! Each row records one (ancestor, descendant, depth) triple.  A node always
//! has a self-reference row with `depth = 0`.  Together these rows power
//! efficient subtree queries and cycle-prevention checks.

use sea_orm::entity::prelude::*;

/// Row model for one entry in the closure table.
///
/// The composite primary key is `(ancestor_id, descendant_id)`.
#[derive(Clone, Debug, PartialEq, DeriveEntityModel, Eq)]
#[sea_orm(table_name = "file_node_paths")]
pub struct Model {
    /// UUID of an ancestor node (or the node itself when `depth = 0`).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub ancestor_id: String,

    /// UUID of the descendant node (or the node itself when `depth = 0`).
    #[sea_orm(primary_key, auto_increment = false, column_type = "Text")]
    pub descendant_id: String,

    /// Number of edges between ancestor and descendant; 0 = self-reference.
    pub depth: i32,
}

#[derive(Copy, Clone, Debug, EnumIter, DeriveRelation)]
pub enum Relation {}

impl ActiveModelBehavior for ActiveModel {}
