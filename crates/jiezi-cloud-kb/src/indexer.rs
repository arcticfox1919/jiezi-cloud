//! Domain-event subscriber that drives KB indexing.
//!
//! This is the **only** place where `jiezi-cloud-kb` reacts to VFS lifecycle
//! events.  The indexer runs as a background Tokio task at server startup and
//! listens on a `broadcast::Receiver<DomainEvent>`.
//!
//! # Which events are handled
//!
//! | Event                    | Action                                        |
//! |--------------------------|-----------------------------------------------|
//! | `FileUploaded`           | `register` if file is `.md`                   |
//! | `FilePermanentlyDeleted` | `unregister` (clean up KB metadata)           |
//! | `FileMoved`              | `reindex` if the file is a known KB note      |
//!
//! # Events intentionally ignored
//!
//! - `FileDeleted` (soft-delete / trash) — the note is still accessible and
//!   should remain in the index.  Cleanup happens on `FilePermanentlyDeleted`.
//! - All user, share and space events — not KB-relevant.

use std::sync::Arc;

use jiezi_cloud_core::{
    events::DomainEvent,
    traits::{kb::KbService, vfs::VfsService},
    types::FileId,
};
use tokio::sync::broadcast;
use tracing::{debug, error, warn};

/// Spawn the KB indexer as a background Tokio task.
///
/// Receives domain events over `rx` and calls the appropriate [`KbService`]
/// method.  The task runs until the channel is closed (i.e., until the process
/// exits).
///
/// # Usage in `main.rs`
///
/// ```rust,no_run
/// # use std::sync::Arc;
/// # use jiezi_cloud_core::{events::DomainEvent, traits::kb::KbService};
/// # use tokio::sync::broadcast;
/// # let kb: Arc<dyn KbService> = todo!();
/// # let rx: broadcast::Receiver<DomainEvent> = todo!();
/// # let vfs: Arc<dyn jiezi_cloud_core::traits::vfs::VfsService> = todo!();
/// jiezi_cloud_kb::indexer::start(rx, kb, vfs);
/// ```
pub fn start(
    mut rx: broadcast::Receiver<DomainEvent>,
    kb: Arc<dyn KbService>,
    vfs: Arc<dyn VfsService>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        loop {
            match rx.recv().await {
                Ok(event) => handle_event(&event, &kb, &vfs).await,
                Err(broadcast::error::RecvError::Lagged(n)) => {
                    warn!(skipped = n, "KB indexer lagged — some events were missed");
                }
                Err(broadcast::error::RecvError::Closed) => {
                    debug!("KB indexer: domain event channel closed, shutting down");
                    break;
                }
            }
        }
    })
}

async fn handle_event(
    event: &DomainEvent,
    kb: &Arc<dyn KbService>,
    vfs: &Arc<dyn VfsService>,
) {
    match event {
        DomainEvent::FileUploaded { file_id, .. } => {
            if is_markdown_file(file_id, vfs).await {
                if let Err(e) = kb.register(file_id).await {
                    error!(error = %e, file = %file_id, "KB register failed");
                }
            }
        }

        DomainEvent::FilePermanentlyDeleted { file_id, .. } => {
            if let Err(e) = kb.unregister(file_id).await {
                error!(error = %e, file = %file_id, "KB unregister failed");
            }
        }

        DomainEvent::FileMoved { file_id, .. } => {
            // Re-index on move: the VFS path may appear in backlinks or slug
            // generators, and the file name could change.
            if let Err(e) = kb.reindex(file_id).await {
                // A NotFound means this file was never a KB note — ignore.
                if !matches!(e, jiezi_cloud_core::error::AppError::NotFound(_)) {
                    error!(error = %e, file = %file_id, "KB reindex on move failed");
                }
            }
        }

        _ => {} // All other events are not KB-relevant.
    }
}

/// Return `true` if the VFS node for `file_id` looks like a Markdown file.
async fn is_markdown_file(file_id: &FileId, vfs: &Arc<dyn VfsService>) -> bool {
    match vfs.get_node(file_id).await {
        Ok(node) => {
            node.mime_type.as_deref() == Some("text/markdown")
                || node.name.ends_with(".md")
                || node.name.ends_with(".markdown")
        }
        Err(e) => {
            warn!(error = %e, file = %file_id, "KB indexer could not fetch node metadata");
            false
        }
    }
}
