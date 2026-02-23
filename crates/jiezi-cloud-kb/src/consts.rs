//! Compile-time constants shared across the knowledge-base layer.

/// Name of the hidden root directory that hosts the knowledge base within a
/// space's VFS tree.
///
/// Every space that opts into KB features has a `.knowledge-base/` directory
/// at the top level of its VFS root.  The leading dot makes it hidden in most
/// directory-listing UIs.
pub const KB_ROOT_DIR: &str = ".knowledge-base";

/// Name of the per-group assets sub-directory.
///
/// Created automatically inside each note group folder, e.g.:
/// `.knowledge-base/Daily Notes/assets/`
///
/// Image files uploaded via the KB editor are stored here so that VFS
/// browsing shows them alongside the notes that reference them.
pub const KB_ASSETS_DIR: &str = "assets";

/// Name of the space-level shared-assets directory.
///
/// Located directly under [`KB_ROOT_DIR`] (`.knowledge-base/.assets/`).
/// The leading dot hides it from the "list groups" query and signals that it
/// is not itself a note group.
pub const KB_SPACE_ASSETS_DIR: &str = ".assets";

/// URI scheme prefix for embedded asset images referenced inside Markdown.
///
/// A complete asset reference looks like `jiezi://asset/<file_uuid>`.  These
/// URIs are:
/// - **Stable** — they survive domain/hostname changes.
/// - **Access-controlled** — the server validates permissions at render time.
/// - **Compact** — only the UUID is stored, not a full URL.
///
/// At render time (`KbService::render_note`) these are rewritten to ordinary
/// HTTP download URLs served by the Jiezi Cloud server.
pub const JIEZI_ASSET_SCHEME: &str = "jiezi://asset/";
