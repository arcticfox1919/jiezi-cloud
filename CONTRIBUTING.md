# Jiezi Cloud — Contributing Guide & Code Standards

## Table of Contents

- [Repository Structure](#repository-structure)
- [Commit Convention](#commit-convention)
- [Code Style](#code-style)
- [Testing Standards](#testing-standards)
- [Adding a Storage Backend](#adding-a-storage-backend)

---

## Repository Structure

```
jiezi-cloud/
├── crates/
│   ├── jiezi-cloud-core/        # Domain models, error types, shared traits
│   ├── jiezi-cloud-storage/     # Storage backend implementations (local / S3 / WebDAV)
│   ├── jiezi-cloud-auth/        # JWT, password hashing, RBAC
│   ├── jiezi-cloud-vfs/         # Virtual filesystem (closure table + metadata)
│   ├── jiezi-cloud-migration/   # SeaORM database migrations
│   └── ...
├── Cargo.toml                   # Workspace root — centralised dependency versions
└── CONTRIBUTING.md              # This file
```

---

## Commit Convention

Follow [Conventional Commits](https://www.conventionalcommits.org/):

```
feat(storage): add S3 backend with multipart upload
fix(auth): reject empty passwords early
refactor(vfs): extract closure-table helpers
test(core): add BackendDriver round-trip tests
docs: update CONTRIBUTING with test convention
```

---

## Code Style

- Format with `cargo fmt` before every commit.
- `cargo clippy -- -D warnings` must produce zero warnings.
- All public APIs require doc comments (`///`); module top-level uses `//!`.
- `unsafe` code is forbidden (workspace lint `unsafe_code = "forbid"`).
- All errors flow through `jiezi_cloud_core::error::{AppError, AppResult}`.
  `.unwrap()` is banned in non-test code.

---

## Testing Standards

### Preferred: `tests/` directory (public API tests)

Place unit tests in the crate's `tests/` directory whenever possible.
Files in `tests/` are compiled as separate crates and can only access **public
API** — which is the same guarantee callers have.  If you cannot test a
behaviour through the public interface, that is a signal to improve the design.

**Structure:**

```
crates/jiezi-cloud-core/
└── tests/
    ├── test_error.rs
    ├── test_events.rs
    ├── test_types.rs
    ├── test_models_chunk.rs
    ├── test_models_file.rs
    ├── test_models_share.rs
    ├── test_models_space.rs
    ├── test_models_user.rs
    ├── test_models_task.rs
    ├── test_models_backend.rs
    └── test_protocol_codec.rs
```

Each file imports the crate by its lib name:

```rust
use jiezi_cloud_core::models::chunk::ChunkInfo;
```

Add required crates (e.g. `bytes`, `uuid`, `chrono`) to `[dev-dependencies]` in
the crate's `Cargo.toml`.

### Exception: sibling `*_tests.rs` for private-access tests

When a test **must** access private fields or free functions, use a sibling
`<module>_tests.rs` file declared at the bottom of the source file:

```rust
// src/foo.rs — one line at the bottom
#[cfg(test)]
#[path = "foo_tests.rs"]
mod tests;
```

> The `#[path]` attribute is required because `foo.rs` is a flat file, not a
> directory module.  Without it rustc looks for `foo/tests.rs` and fails.

This is the **exception**, not the rule.  Prefer designing APIs so tests do not
need private access.

### Integration / I/O tests

Tests that span multiple modules or require real I/O also go in `tests/`.
Name files after the feature under test:

```
crates/jiezi-cloud-storage/
└── tests/
    ├── local_backend_integration.rs
    └── s3_backend_integration.rs
```

### Test helpers

- Temporary directories: use the `tempfile` crate (already in `storage`'s `[dev-dependencies]`).
- Async tests: annotate with `#[tokio::test]`.
- Mocking: prefer a Trait + in-memory fake over heavy mocking frameworks.

---

## Adding a Storage Backend

Backends use an **open driver-string** design (see
`jiezi-cloud-core::models::backend`).  Adding a new driver requires **no
changes to the core model and no schema migrations**:

1. Add to `jiezi-cloud-core/src/models/backend.rs`:
   - A `pub const DRIVER_XXXX: &str = "xxxx";` constant.
   - A typed config struct (e.g. `XxxxConfig`).
2. Create `jiezi-cloud-storage/src/xxxx.rs` implementing the `StorageBackend` trait.
3. Register the driver in the `StorageManager` backend factory.

Currently shipped built-in drivers:

| Constant | Kind string | Description |
|----------|-------------|-------------|
| `DRIVER_LOCAL_FS` | `"local_fs"` | Local filesystem directory on the server host |
| `DRIVER_S3_COMPATIBLE` | `"s3_compatible"` | Any S3-compatible service (AWS S3, MinIO, Aliyun OSS, Tencent COS, Cloudflare R2 …) |
| `DRIVER_WEBDAV` | `"webdav"` | WebDAV server (Nextcloud, ownCloud, nginx dav …) |

Third-party or user-contributed drivers may use any string kind and do not need
to be listed here.
