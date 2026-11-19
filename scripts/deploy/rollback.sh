#!/usr/bin/env bash
# Quick rollback: flips traffic back to the previous slot immediately.
#
# Fast path: if the previous slot is still warm (within
# ROLLBACK_WINDOW_SECONDS of the last deploy, before blue-green-deploy.sh
# decommissioned it), this is just an nginx reload — seconds, not a
# redeploy.
# Slow path: if the previous slot was already stopped, this restarts it
# on whatever image tag it last ran (recorded in deploy/state/*-image)
# before flipping traffic, which takes longer but still doesn't require
# rebuilding anything..
#
# Usage: ./scripts/deploy/rollback.sh
set -euo pipefail

cd "$(dirname "${BASH_SOURCE[0]}")/../.."

compose="docker compose -f deploy/docker-compose.deploy.yml"
state_dir="deploy/state"
active_file="$state_dir/active-slot"
previous_file="$state_dir/previous-slot"

[ -f "$active_file" ] || { echo "No active-slot state found — nothing to roll back from." >&2; exit 1; }
[ -f "$previous_file" ] || { echo "No previous-slot recorded — nothing to roll back to." >&2; exit 1; }

current_color="$(cat "$active_file")"
rollback_color="$(cat "$previous_file")"

if [ "$current_color" = "$rollback_color" ]; then
  echo "Active slot already equals the recorded previous slot ($current_color) — nothing to do." >&2
  exit 0
fi

echo "Rolling back: $current_color -> $rollback_color"

backend_image_file="$state_dir/$rollback_color-backend-image"
frontend_image_file="$state_dir/$rollback_color-frontend-image"
revision_file="$state_dir/$rollback_color-source-revision"
verification_run_file="$state_dir/$rollback_color-verification-run-id"
provenance_file="$state_dir/$rollback_color-provenance-verified"
[ -s "$backend_image_file" ] && [ -s "$frontend_image_file" ] \
  && [ -s "$revision_file" ] && [ -s "$verification_run_file" ] \
  && [ -s "$provenance_file" ] || {
  echo "Rollback target lacks recorded image digests or provenance evidence; refusing to restore it." >&2
  exit 1
}
[ "$(cat "$provenance_file")" = "true" ] || {
  echo "Rollback target provenance was not verified; refusing to restore it." >&2
  exit 1
}
rollback_backend_image="$(cat "$backend_image_file")"
rollback_frontend_image="$(cat "$frontend_image_file")"
rollback_revision="$(cat "$revision_file")"
verification_run_id="$(cat "$verification_run_file")"
for image_ref in "$rollback_backend_image" "$rollback_frontend_image"; do
  [[ "$image_ref" =~ @sha256:[0-9a-f]{64}$ ]] || {
    echo "Rollback target has a non-digest image reference: $image_ref" >&2
    exit 1
  }
done

if ! $compose ps --status running "backend-$rollback_color" | grep -q "backend-$rollback_color"; then
  echo "$rollback_color slot is stopped — starting it back up before cutover"
  BACKEND_IMAGE_REF="$rollback_backend_image" \
  FRONTEND_IMAGE_REF="$rollback_frontend_image" \
    $compose up -d "backend-$rollback_color" "frontend-$rollback_color"
  for attempt in $(seq 1 10); do
    if $compose exec -T "backend-$rollback_color" curl -fsS "http://localhost:8080/api/health" >/dev/null 2>&1; then
      break
    fi
    [ "$attempt" -eq 10 ] && { echo "Rollback target $rollback_color failed to become healthy." >&2; exit 1; }
    sleep 3
  done
fi

./scripts/deploy/render-upstream.sh "$rollback_color"
echo "$rollback_color" > "$active_file"
echo "$current_color" > "$previous_file"

end_iso=$(date -u +%Y-%m-%dT%H:%M:%SZ)
printf '{"finished_at":"%s","from_slot":"%s","to_slot":"%s","source_revision":"%s","verification_run_id":"%s","backend_image":"%s","frontend_image":"%s","provenance_verified":true,"status":"rollback"}\n' \
  "$end_iso" "$current_color" "$rollback_color" "$rollback_revision" "$verification_run_id" \
  "$rollback_backend_image" "$rollback_frontend_image" >> deploy/metrics/deployments.jsonl

echo "Rollback complete. Active slot: $rollback_color"
