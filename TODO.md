# Jiezi Cloud — Project TODO

> Tracking implementation progress against the architecture defined in [ARCHITECTURE.md](ARCHITECTURE.md).
> Check a box when the feature is fully implemented and passing tests.

---

## Stage 0 — Workspace Skeleton & CI

- [x] Cargo workspace with all crates scaffolded
- [x] Shared dependency versions in root `Cargo.toml`
- [x] GitHub Actions CI pipeline (`cargo build`, `cargo test`)
- [x] `config/` directory with `default.toml` and environment overlays

---

## Stage 1 — Core Types, Traits & Error Handling

- [x] `jiezi-cloud-core`: domain types (`UserId`, `FileNodeId`, `SpaceId`, …)
- [x] `jiezi-cloud-core`: unified `AppError` / `AppResult` with `thiserror`
- [x] `jiezi-cloud-core`: trait definitions (`StorageBackend`, `VfsService`, `AuthService`, …)
- [x] `jiezi-cloud-core`: domain events (`DomainEvent` enum)
- [x] `jiezi-cloud-core`: shared model structs (`FileNode`, `Space`, `User`, …)

---

## Stage 2 — Authentication & Authorization

- [x] Argon2id password hashing (`jiezi-cloud-auth`)
- [x] ES256 JWT access tokens (ECDSA P-256, asymmetric: home server signs, tunnel can only verify)
- [x] `JwtVerifier` (public-key-only verifier for use in `jiezi-cloud-tunnel`)
- [x] Auto-generate ephemeral keypair in dev (`jwt_private_key_pem = "GENERATE"`)
- [x] Refresh token rotation with reuse detection
- [x] RBAC roles: Owner / Admin / Member / Guest
- [x] Account lockout after repeated failed logins
- [x] Login rate limiting (middleware)
- [x] Email OTP — register (verify email)
- [x] Email OTP — reset password
- [x] Email OTP — change password
- [x] Email OTP — unlock account
- [x] Auth HTTP routes: `POST /auth/login`, `/auth/register`, `/auth/refresh`, `/auth/me`, OTP endpoints
- [x] JWT extractor middleware for Actix-web

---

## Stage 2.5 — Configuration System

- [x] Layered config: `default.toml` → environment file → local override → env-var overrides
- [x] `jiezi-cloud-config` crate with typed `AppConfig`, `StorageConfig`, `AuthConfig`, …
- [x] `large_file_threshold_bytes = 20971520` (20 MiB — QUIC/HTTP-2 routing threshold)
- [x] Production validation (reject insecure defaults in release mode)

---

## Stage 2.6 — Database Integrity

- [x] SQLite WAL mode enabled at startup
- [x] Startup health check (schema version, connectivity)
- [x] Scheduled auto-backup (configurable interval, file rotation)
- [x] 11 SeaORM migrations: `users`, `refresh_tokens`, `spaces`, `file_nodes`, `file_node_paths`, `storage_backend_configs`, `chunk_locations`, `file_chunks`, `system_settings`, `login_lockout`, `email_verification`

---

## Stage 3 — Storage Backends & Chunking

- [x] `StorageBackend` trait (`put`, `get`, `get_range`, `delete`, `exists`, `health_check`)
- [x] `LocalFsBackend`: atomic write (temp → rename), SHA-256 verify, range reads
- [x] `S3Backend`: AWS SDK v1, put/get/delete/range
- [x] `WebDavBackend`: reqwest-based, basic auth
- [x] `FastCdcChunker`: content-defined chunking (deterministic, stable boundaries)
- [x] `StorageManager`: registry of backends, `put_to_all`, `put_to`, `put_with_policy`, `get_from_any`, `health_check_all`
- [x] `ReplicationPolicy` enum: `All` / `MinN(usize)` / `Specific(Vec<id>)`
- [x] SeaORM entities: `chunk_locations`, `file_chunks`

---

## Stage 4 — Virtual File System (VFS)

- [x] Closure-table schema (`file_node_paths`) for unlimited-depth hierarchy
- [x] `FileNodeRepository`: create, rename, move, copy, soft-delete, restore, permanent-delete, trash listing
- [x] `VfsServiceImpl` wrapping repository behind trait
- [x] `create_file_record(parent_id, name, size, content_hash, mime_type, owner)` on `VfsService`
- [x] Space-scoped root nodes

---

## Stage 5 — HTTP/2 API Server

- [x] Actix-web 4 server with Tokio runtime
- [x] Middleware stack: tracing, CORS, JWT auth extraction, rate limiting, first-run setup guard
- [x] VFS CRUD routes (`GET/POST/PATCH/DELETE /api/v1/files/…`)
- [x] Upload route: `POST /api/v1/upload/` (raw body + query params)
- [x] Download route: `GET /api/v1/download/{id}` (full + `Range` → 206)
- [x] Admin routes: `GET/POST /api/v1/admin/…`
- [x] Setup route: `POST /api/v1/setup` (first-run admin creation)
- [x] SSE endpoint: `GET /api/v1/events` (server-sent events bus)
- [x] `AppState` wired: auth + vfs + storage + upload + download services

---

## Stage 7 — Upload & Download Pipeline

- [x] `UploadService::store_file`: FastCDC → `put_with_policy` → write `file_chunks` + `chunk_locations`
- [x] Content-hash deduplication (skip re-upload if chunk already exists)
- [x] `DownloadService::read_file`: full reassembly from DB chunk list → backends
- [x] `DownloadService::read_range`: byte-range slicing across chunk boundaries
- [x] `StoredFileInfo` returned from upload (file_node_id, content_hash, size, chunk_count)

---

## Stage 6 — QUIC File Transfer Server

- [x] Add `quinn` dependency to `jiezi-cloud-server`
- [x] TLS certificate loading (self-signed for dev, ACME for prod)
- [x] QUIC listener on UDP port 4433 (`QuicServer::new` + `run`)
- [x] `handle_upload_stream`: receive bidirectional stream, write to `UploadService`
- [x] `handle_download_stream`: open unidirectional stream, stream bytes from `DownloadService`
- [x] JTP/1 protocol: parallel chunk streams + sliding-window (`parallel_streams`, `window_size` negotiated in `UPLOAD_ACCEPT` / `DOWNLOAD_INFO`)
- [x] `JtpTransport` abstraction trait (`max_parallel_streams`: QUIC=8, WebSocket=1, test=1)
- [x] Route files ≥ 20 MiB to QUIC transport: native clients → HTTP 426 `QUIC_REQUIRED`; web clients → HTTP 413 `FILE_TOO_LARGE_FOR_WEB`
- [x] `TunnelConfig` + per-client web upload limits (`web_upload_max_bytes_no_tunnel` 500 MiB, `web_upload_max_bytes_with_tunnel` 100 MiB)
- [x] `AppState` routing fields (`quic_port`, `tunnel_enabled`, `large_file_threshold`, web limits)
- [x] WebSocket fallback transport (`WsTransport` — `!Send`, `actix_rt::spawn`) + `GET /api/v1/transfer/ws`
- [x] `run_ws_session` + `dispatch_ws_frame` in `quic/connection.rs` (inline HELLO handshake for WS path)
- [ ] QUIC loopback integration tests (full upload/download round-trip via `ChannelTransport`)
- [x] JTP/1 session unit tests via `ChannelTransport` (handshake: valid token, version mismatch, invalid JWT, peer closed)

---

## Stage 7.5 — Resumable Upload Sessions

- [ ] `upload_sessions` DB table + migration (id, user_id, file_name, total_size, content_hash, chunk_statuses, created_at, expires_at)
- [ ] `UploadSessionRepository` + SeaORM entity
- [ ] `POST /api/v1/upload/prepare` — create session, return session_id; instant-upload check (dedup by content_hash)
- [ ] `GET /api/v1/upload/{session_id}/status` — return missing chunk indices
- [ ] `POST /api/v1/upload/{session_id}/chunk/{index}` — upload individual chunk
- [ ] `POST /api/v1/upload/{session_id}/complete` — assemble file, create VFS record
- [ ] `DELETE /api/v1/upload/{session_id}` — cancel and clean up
- [ ] Session expiry background job (purge stale sessions)
- [ ] Tests: prepare → partial upload → resume → complete lifecycle

---

## Stage 8 — Download Tokens & Streaming

- [ ] `download_tokens` DB table + migration (token, file_node_id, user_id, expires_at, one_time)
- [ ] `POST /api/v1/download/{id}/token` — generate time-limited download token
- [ ] Token validation middleware for unauthenticated download URLs
- [ ] QUIC download uses token for auth (no JWT header over QUIC stream)
- [ ] Streaming download with back-pressure (no full buffer in memory)
- [ ] Tests: token generation, expiry, one-time use, invalid token rejection

---

## Stage 9 — Full-Text Search

- [ ] `jiezi-cloud-search` crate: Tantivy index (title, body, mime_type, owner_id)
- [ ] `jieba-rs` Chinese tokenizer integrated as Tantivy tokenizer
- [ ] `SearchIndexer`: index on file upload event, update on rename, delete on remove
- [ ] Text extraction pipeline: PDF (`pdf-extract`), plain text / Markdown, fallback → `None`
- [ ] `SearchService::query(q, filters, page, size)` → ranked results with highlight snippets
- [ ] `GET /api/v1/search?q=…&type=…&page=…&size=…`
- [ ] Async index update triggered by domain events (not blocking upload)
- [ ] Tests: Chinese query, English query, mixed, filter by type, pagination, highlight

---

## Stage 10 — Online Preview

- [ ] `jiezi-cloud-preview` crate
- [ ] Thumbnail generation: JPEG/PNG via `image` crate, configurable max size
- [ ] PDF preview: serve original PDF (front-end uses pdf.js); validate Content-Type
- [ ] Video HLS transcoding: FFmpeg wrapper → `m3u8` + `.ts` segments
- [ ] Thumbnail disk cache (skip regeneration on second request)
- [ ] `GET /api/v1/preview/{id}` — auto-route by MIME type
- [ ] `GET /api/v1/preview/{id}/thumbnail` — thumbnail image
- [ ] `GET /api/v1/preview/{id}/stream` — HLS playlist
- [ ] Tests: correct MIME on response, unknown type → 415, cache hit behaviour

---

## Stage 11 — Flutter Web MVP

- [ ] `client/` Flutter project (`--platforms=web`): Riverpod, go_router, Dio, Material 3
- [ ] Login page (username + password)
- [ ] File list page (directory tree, breadcrumb navigation)
- [ ] File upload (drag-and-drop, progress bar, multi-file)
- [ ] File operations context menu (rename / move / delete)
- [ ] File preview modal (image / PDF / text)
- [ ] Search bar + results list
- [ ] File download button
- [ ] WebTransport integration for QUIC uploads (auto-fallback to HTTP/2 chunked)
- [ ] Widget tests: form validation, file list render (mock API), upload progress, search results

---

## Stage 12 — Docker Deployment & Integration Tests

- [ ] Multi-stage `Dockerfile` (Rust builder → debian-slim runtime)
- [ ] `docker-compose.yml` exposing ports 8080 (HTTP), 8443 (HTTPS), 4433/udp (QUIC)
- [ ] Volume mount for `/data` (SQLite + local storage root)
- [ ] First-run setup wizard validated in Docker environment
- [ ] E2E test suite (`tests/e2e/`): register → login → mkdir → upload (QUIC) → search → preview → download (verify SHA-256) → delete
- [ ] `cargo test --test e2e` passes against running Docker instance
- [ ] CI job: build image, run E2E tests, report coverage

---

## Stage 13 — Share Links & Collaboration Spaces

- [ ] `jiezi-cloud-share` crate
- [ ] Share link creation: generate token, store expiry + optional password + download limit
- [ ] `GET /api/v1/share/{token}` — public file access via share token
- [ ] Password-protected share validation
- [ ] Download count enforcement
- [ ] Share revocation (`DELETE /api/v1/share/{token}`)
- [ ] Collaboration space: create space, invite members, assign roles
- [ ] Space member CRUD API (`POST/DELETE /api/v1/spaces/{id}/members`)
- [ ] SSE notifications for share events and space file changes (broadcast channel per user)
- [ ] `Last-Event-ID` reconnect support (replay missed events)
- [ ] Tests: full share lifecycle, password enforcement, expiry, member permission gates

---

## Stage 14 — Distributed Storage Backends

- [ ] `storage_backend_configs` repository: CRUD ORM entity + `BackendRepository` trait impl
- [ ] Admin REST API: `GET/POST/PATCH/DELETE /api/v1/admin/backends`
- [ ] S3Backend: complete (MinIO integration tests with real bucket)
- [ ] WebDavBackend: complete (digest auth, large file chunked PUT)
- [ ] `PolicyEngine`: rule-based backend selection (by file type, size, tags, space)
- [ ] `StoragePlan` with explicit backend list + replication factor
- [ ] Async replica sync task (retry on failure, exponential back-off)
- [ ] Consistency scanner: periodic SHA-256 re-verification across backends
- [ ] Tests: policy matching, multi-replica creation, failure + retry, consistency repair

---

## Stage 15 — AI Module

- [ ] `jiezi-cloud-ai` crate
- [ ] `LlmProvider` trait: `chat(messages)` + `embedding(text)`
- [ ] `OllamaProvider` impl (local model, configurable endpoint)
- [ ] `OpenAiProvider` impl (API key, model selection)
- [ ] Document vectorisation pipeline: text chunk → embedding → vector index
- [ ] RAG retrieval: semantic search → top-k chunks → prompt construction → LLM answer
- [ ] `POST /api/v1/ai/chat` — conversational Q&A with RAG context
- [ ] `POST /api/v1/ai/summarize` — per-document summary
- [ ] `POST /api/v1/ai/search` — semantic search (beyond keyword matching)
- [ ] AI features gated by `ai.enabled = true` config flag (core functions unaffected when off)
- [ ] Tests (mock provider): prompt building, embedding dimension, RAG context inclusion, source citations

---

## Stage 16 — Multi-Platform Clients & Cluster Deployment

- [ ] Flutter iOS / Android client (photo auto-backup, `flutter_rust_bridge` QUIC integration)
- [ ] Flutter Windows / macOS / Linux desktop client (local folder sync)
- [ ] PostgreSQL backend: `Database` trait impl for PG, feature flag `db-postgres`
- [ ] SQLite → PostgreSQL migration utility + tests
- [ ] All existing DB tests passing on PostgreSQL
- [ ] Multi-node deployment: stateless API nodes behind Nginx
- [ ] PostgreSQL primary/replica replication setup (docker-compose cluster variant)
- [ ] Tantivy search index sync across nodes
- `jiezi-cloud-tunnel` binary: WSS control channel, HMAC auth, public-key push (`jwt_public_key` in `RegisterNode`), relay fallback, wildcard subdomain proxy

---

## Ongoing / Cross-Cutting

- [ ] Unit test coverage ≥ 85% overall (targets per crate in `工程实施文档.md` Appendix A)
- [ ] `cargo clippy -- -D warnings` clean on CI
- [ ] `cargo fmt --check` enforced on CI
- [ ] API contract documented (OpenAPI / manual)
- [ ] `ARCHITECTURE.md` kept current with each significant change
- [ ] Performance baseline: search < 100 ms on 1 000 documents; download throughput ≥ 100 MiB/s on loopback
