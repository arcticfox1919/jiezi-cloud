#!/usr/bin/env bash
# Jiezi Cloud — Server launcher
#
# Usage:
#   scripts/start.sh                  # debug + SQLite (default)
#   scripts/start.sh -r               # release build
#   scripts/start.sh -c               # clean data/ before starting
#   scripts/start.sh db=postgres      # debug + PostgreSQL
#   scripts/start.sh db=mysql         # debug + MySQL
#   scripts/start.sh -r db=postgres   # release + PostgreSQL
#   scripts/start.sh -c -r db=sqlite  # clean + release + SQLite

set -euo pipefail

# Resolve workspace root
cd "$(dirname "$0")/.."

# ── Parse arguments ────────────────────────────────────────────────────────────
RELEASE_FLAG=""
DB_FEATURE="db-sqlite"
CLEAN_DATA=""

while [[ $# -gt 0 ]]; do
    case "$1" in
        -r) RELEASE_FLAG="--release" ;;
        -c) CLEAN_DATA="1" ;;
        db=*)
            db_val="${1#db=}"
            case "$db_val" in
                sqlite|postgres|mysql) DB_FEATURE="db-${db_val}" ;;
                *) echo "Unknown db value: ${db_val}" >&2; exit 1 ;;
            esac
            ;;
        *) echo "Unknown argument: $1" >&2; exit 1 ;;
    esac
    shift
done

# Clean data directory if requested
if [[ -n "$CLEAN_DATA" && -d data ]]; then
    rm -rf data
fi

# Stop any running instance to release file locks
if pgrep -x jiezi-cloud-server > /dev/null 2>&1; then
    pkill -x jiezi-cloud-server
    sleep 1
fi

mkdir -p data

# shellcheck disable=SC2086
cargo run --bin jiezi-cloud-server --no-default-features --features "$DB_FEATURE" $RELEASE_FLAG
