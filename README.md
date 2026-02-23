# Jiezi Cloud

A self-hosted cloud drive service built with Rust / Actix-web + QUIC.

---

## Quick Start (Development)

### Prerequisites

| Tool  | Version                                     |
|-------|---------------------------------------------|
| Rust  | stable (run `rustup update stable` to upgrade) |
| Cargo | Included with Rust                          |

SQLite is embedded as the default database — **no external database server required**.

---

### 1. Clone the Repository

```bash
git clone https://github.com/your-org/jiezi-cloud.git
cd jiezi-cloud
```

### 2. Local Configuration (Optional)

`config/local.toml` can override any default setting and is already listed in `.gitignore`, making it a safe place for local secrets:

```bash
# Example: change the listening port
cat > config/local.toml << 'EOF'
[server]
port = 9090
EOF
```

Development defaults (from `config/default.toml`):

| Setting               | Default Value                                       |
|-----------------------|-----------------------------------------------------|
| Listen address        | `127.0.0.1:8080`                                    |
| Database              | `./data/jiezi-dev.db` (SQLite, auto-created)        |
| JWT signing key       | `GENERATE` (randomly generated on each start, dev only) |
| File storage root     | `./data/files`                                      |

### 3. Build and Run the Server

```bash
cargo run -p jiezi-cloud-server
```

> **Run from the project root** (the directory containing `Cargo.toml`), otherwise relative paths like `./data/jiezi-dev.db` will not resolve correctly.
>
> **On first launch**, the server automatically:
> 1. Creates the `data/` directory (if missing) and the SQLite database file
> 2. Applies WAL mode and `synchronous=FULL` durability pragmas
> 3. Runs all pending schema migrations (via SeaORM)
> 4. Binds the HTTP port after startup integrity checks pass

Once running, you can access:

- **REST API**: `http://127.0.0.1:8080/api/`
- **Swagger UI**: `http://127.0.0.1:8080/swagger-ui/`

---

### 4. Running Tests

```bash
# Run all tests
cargo test

# Run tests for a specific crate
cargo test -p jiezi-cloud-auth
```

---

## Using PostgreSQL (Production / Optional)

1. Create the database:

```sql
CREATE DATABASE jiezi_cloud;
```

2. Build with the PostgreSQL feature and provide the connection string:

```bash
JIEZI__DATABASE__URL="postgres://user:pass@localhost/jiezi_cloud" \
cargo run -p jiezi-cloud-server --no-default-features --features db-postgres
```

Migrations are applied automatically on first startup, same as with SQLite.

---

## Configuration Precedence (Highest to Lowest)

```
Environment variable JIEZI__SECTION__KEY
config/local.toml       ← local overrides, not committed to git
config/development.toml ← development environment overrides
config/default.toml     ← baseline defaults for all environments
```

Environment variable naming convention: join hierarchy levels with `__`, for example:

```bash
JIEZI__SERVER__PORT=9090
JIEZI__AUTH__JWT_PRIVATE_KEY_PEM="$(cat private.pem)"
```
