#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FIXTURE_DIR="$ROOT_DIR/migration-compatibility"
SCHEMA="migration_compatibility_$$"

: "${DATABASE_URL:?DATABASE_URL must point to a disposable PostgreSQL database}"

psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "CREATE SCHEMA $SCHEMA"
cleanup() {
  psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -c "DROP SCHEMA IF EXISTS $SCHEMA CASCADE" >/dev/null
}
trap cleanup EXIT

psql_fixture() {
  PGOPTIONS="-c search_path=$SCHEMA" \
    psql "$DATABASE_URL" -v ON_ERROR_STOP=1 -f "$1"
}

echo "Checking previous application against the pre-expand schema"
psql_fixture "$FIXTURE_DIR/000_previous_schema.sql"
psql_fixture "$FIXTURE_DIR/previous-app.sql"

echo "Applying expand; previous application remains compatible"
psql_fixture "$FIXTURE_DIR/001_expand.sql"
psql_fixture "$FIXTURE_DIR/previous-app.sql"

echo "Applying backfill; previous and current applications remain compatible"
psql_fixture "$FIXTURE_DIR/002_backfill.sql"
psql_fixture "$FIXTURE_DIR/previous-app.sql"
psql_fixture "$FIXTURE_DIR/current-app.sql"

echo "Verifying contract is blocked without the retirement marker"
apply_contract() {
  if [ "${CONTRACT_RELEASE_MARKER:-}" != "previous-app-retired" ]; then
    echo "Contract migration requires CONTRACT_RELEASE_MARKER=previous-app-retired" >&2
    return 1
  fi
  psql_fixture "$FIXTURE_DIR/003_contract.sql"
}

if CONTRACT_RELEASE_MARKER="" apply_contract >/dev/null 2>&1; then
  echo "Contract migration unexpectedly ran without a retirement marker" >&2
  exit 1
fi

if [ "${CONTRACT_RELEASE_MARKER:-}" != "previous-app-retired" ]; then
  echo "Set CONTRACT_RELEASE_MARKER=previous-app-retired only after retiring the previous application." >&2
  exit 1
fi

echo "Applying contract after the previous application retirement marker"
CONTRACT_RELEASE_MARKER="$CONTRACT_RELEASE_MARKER" apply_contract
psql_fixture "$FIXTURE_DIR/current-app-after-contract.sql"
