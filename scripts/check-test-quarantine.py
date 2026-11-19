#!/usr/bin/env python3
"""Validate, execute, and report explicitly quarantined tests."""

import argparse
import datetime as dt
import json
import os
from pathlib import Path
import subprocess
import sys


ROOT = Path(__file__).resolve().parents[1]
REQUIRED_FIELDS = {
    "test_id",
    "owner",
    "issue",
    "failure_evidence",
    "scope",
    "expires_on",
    "command",
}


def fail(message: str) -> None:
    print(f"::error::{message}", file=sys.stderr)
    raise SystemExit(1)


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument(
        "--manifest",
        type=Path,
        default=ROOT / ".github/test-quarantine.json",
    )
    parser.add_argument(
        "--history",
        type=Path,
        default=ROOT / ".github/test-quarantine-history.json",
    )
    args = parser.parse_args()

    try:
        manifest = json.loads(args.manifest.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        fail(f"Cannot read quarantine manifest {args.manifest}: {error}")

    entries = manifest.get("quarantines")
    if not isinstance(entries, list):
        fail("Quarantine manifest must contain a 'quarantines' array")

    try:
        history = json.loads(args.history.read_text(encoding="utf-8"))
    except FileNotFoundError:
        history = {}
    except (OSError, json.JSONDecodeError) as error:
        fail(f"Cannot read quarantine history {args.history}: {error}")
    if not isinstance(history, dict):
        fail("Quarantine history must be a JSON object")

    today = dt.date.today()
    seen = set()
    results = []
    failures = []

    for entry in entries:
        if not isinstance(entry, dict):
            fail("Each quarantine entry must be an object")
        missing = REQUIRED_FIELDS - entry.keys()
        if missing:
            fail(f"Quarantine entry is missing fields: {', '.join(sorted(missing))}")

        test_id = entry["test_id"]
        if not isinstance(test_id, str) or not test_id.strip() or test_id in seen:
            fail(f"Quarantine test_id must be non-empty and unique: {test_id!r}")
        seen.add(test_id)

        for field in ("owner", "issue", "failure_evidence", "scope"):
            if not isinstance(entry[field], str) or not entry[field].strip():
                fail(f"{test_id}: {field} must be a non-empty string")
        if not entry["issue"].startswith("#") or not entry["issue"][1:].isdigit():
            fail(f"{test_id}: issue must be a repository issue reference such as #123")

        try:
            expires_on = dt.date.fromisoformat(entry["expires_on"])
        except (TypeError, ValueError):
            fail(f"{test_id}: expires_on must be an ISO date (YYYY-MM-DD)")
        if expires_on < today:
            fail(f"{test_id}: quarantine expired on {expires_on.isoformat()}")

        command = entry["command"]
        if (
            not isinstance(command, list)
            or not command
            or any(not isinstance(part, str) or not part for part in command)
        ):
            fail(f"{test_id}: command must be a non-empty argument array")

        streak_target = entry.get("passes_to_clear", 3)
        if not isinstance(streak_target, int) or streak_target < 1:
            fail(f"{test_id}: passes_to_clear must be a positive integer")

        completed = subprocess.run(command, cwd=ROOT, check=False)
        streak = int(history.get(test_id, 0)) + 1 if completed.returncode == 0 else 0
        history[test_id] = streak
        if streak >= streak_target:
            failures.append(
                f"{test_id}: passed {streak} consecutive runs; remove its quarantine"
            )
        result = (
            f"PASS {streak}/{streak_target} consecutive runs; still quarantined"
            if completed.returncode == 0
            else f"DISABLED COVERAGE; isolated run failed (exit {completed.returncode})"
        )
        results.append((test_id, entry["owner"], entry["expires_on"], result))

    try:
        args.history.parent.mkdir(parents=True, exist_ok=True)
        args.history.write_text(json.dumps(history, indent=2) + "\n", encoding="utf-8")
    except OSError as error:
        fail(f"Cannot persist quarantine recovery history: {error}")

    lines = [
        "## Test quarantine report",
        "",
        f"Passing tests: reported by their normal test jobs.",
        f"Disabled/quarantined tests: {len(entries)} (reported separately; never counted as passing).",
        "",
    ]
    for test_id, owner, expires_on, result in results:
        lines.append(f"- `{test_id}` — owner `{owner}`, expires {expires_on}: {result}")
    report = "\n".join(lines) + "\n"
    print(report, end="")
    summary = os.environ.get("GITHUB_STEP_SUMMARY")
    if summary:
        try:
            with open(summary, "a", encoding="utf-8") as summary_file:
                summary_file.write(report)
        except OSError as error:
            fail(f"Cannot write quarantine report to GitHub job summary: {error}")

    if failures:
        fail("; ".join(failures))


if __name__ == "__main__":
    main()
