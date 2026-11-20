#!/bin/bash
# Check for #[contractevent] usage outside arenax-events
VIOLATIONS=$(grep -r '#\[contractevent' contracts/ --include="*.rs" -l | grep -v 'contracts/arenax-events/')

if [ -n "$VIOLATIONS" ]; then
    echo "ERROR: #[contractevent] found outside contracts/arenax-events/:"
    echo "$VIOLATIONS"
    echo ""
    echo "All event definitions must be in the arenax-events shared crate."
    echo "See docs/event-versioning.md for details."
    exit 1
fi

echo "OK: All #[contractevent] definitions are in arenax-events"

# ---------------------------------------------------------------------------
# Analytics event schema registry validation (#249)
#
# Validates fixtures/events/valid/*.json (must pass) and
# fixtures/events/invalid/*.json (must fail) against the shared schema
# registry validator. Also enforces that breaking changes bump the schema
# version by comparing against the committed baseline.
# ---------------------------------------------------------------------------

FIXTURES_DIR="fixtures/events"
VALID_DIR="$FIXTURES_DIR/valid"
INVALID_DIR="$FIXTURES_DIR/invalid"
REGISTRY_DIR="packages/analytics-events"
VALIDATOR="$REGISTRY_DIR/validate-fixture.mjs"
BASELINE="$REGISTRY_DIR/schema-baseline.json"
REGISTRY="$REGISTRY_DIR/registry.json"

mkdir -p "$VALID_DIR" "$INVALID_DIR"

# Seed sample fixtures if the directories are empty so CI always has coverage.
if [ -z "$(ls -A "$VALID_DIR" 2>/dev/null)" ]; then
    cat > "$VALID_DIR/checkout_completed.frontend.json" <<'JSON'
{
  "producer": "frontend",
  "event": "checkout_completed",
  "schemaVersion": 1,
  "payload": {
    "orderId": "ord_123",
    "totalCents": 4999,
    "currency": "USD"
  }
}
JSON
    cat > "$VALID_DIR/checkout_completed.server.json" <<'JSON'
{
  "producer": "server",
  "event": "checkout_completed",
  "schemaVersion": 1,
  "payload": {
    "orderId": "ord_123",
    "totalCents": 4999,
    "currency": "USD",
    "settledAt": "2024-01-01T00:00:00Z"
  }
}
JSON
fi

if [ -z "$(ls -A "$INVALID_DIR" 2>/dev/null)" ]; then
    cat > "$INVALID_DIR/checkout_completed.missing_required.json" <<'JSON'
{
  "producer": "frontend",
  "event": "checkout_completed",
  "schemaVersion": 1,
  "payload": {
    "orderId": "ord_123"
  }
}
JSON
    cat > "$INVALID_DIR/checkout_completed.sensitive_attribute.json" <<'JSON'
{
  "producer": "server",
  "event": "checkout_completed",
  "schemaVersion": 1,
  "payload": {
    "orderId": "ord_123",
    "totalCents": 4999,
    "currency": "USD",
    "cardNumber": "4111111111111111"
  }
}
JSON
    cat > "$INVALID_DIR/checkout_completed.unknown_event.json" <<'JSON'
{
  "producer": "frontend",
  "event": "not_a_registered_event",
  "schemaVersion": 1,
  "payload": {}
}
JSON
fi

if [ ! -f "$VALIDATOR" ]; then
    echo "ERROR: analytics event fixture validator not found at $VALIDATOR"
    echo "See docs/event-versioning.md for details."
    exit 1
fi

if ! command -v node >/dev/null 2>&1; then
    echo "ERROR: node is required to validate analytics event fixtures"
    exit 1
fi

FAILED=0

# Valid fixtures must pass validation.
for fixture in "$VALID_DIR"/*.json; do
    [ -e "$fixture" ] || continue
    if ! node "$VALIDATOR" --fixture "$fixture" --expect valid; then
        echo "ERROR: valid fixture failed validation: $fixture"
        FAILED=1
    fi
done

# Invalid fixtures must fail validation.
for fixture in "$INVALID_DIR"/*.json; do
    [ -e "$fixture" ] || continue
    if ! node "$VALIDATOR" --fixture "$fixture" --expect invalid; then
        echo "ERROR: invalid fixture passed validation: $fixture"
        FAILED=1
    fi
done

# Enforce that breaking changes bump the schema version.
if [ -f "$BASELINE" ] && [ -f "$REGISTRY" ]; then
    if ! node "$VALIDATOR" --check-breaking --registry "$REGISTRY" --baseline "$BASELINE"; then
        echo "ERROR: breaking schema change detected without a version bump."
        echo "Bump the schema version and document a migration plan in docs/event-versioning.md."
        FAILED=1
    fi
else
    echo "WARN: schema baseline or registry missing; skipping breaking-change check."
fi

if [ "$FAILED" -ne 0 ]; then
    echo ""
    echo "Analytics event fixture validation failed. See docs/event-versioning.md."
    exit 1
fi

echo "OK: Analytics event fixtures validated against schema registry"