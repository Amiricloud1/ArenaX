import json
import subprocess
import sys
import tempfile
import unittest
from pathlib import Path


ROOT = Path(__file__).resolve().parents[2]
VALIDATOR = ROOT / "scripts/check-test-quarantine.py"


class TestQuarantinePolicy(unittest.TestCase):
    def run_validator(self, manifest, history=None):
        with tempfile.TemporaryDirectory() as tmp:
            tmp_path = Path(tmp)
            manifest_path = tmp_path / "manifest.json"
            history_path = tmp_path / "history.json"
            manifest_path.write_text(json.dumps(manifest), encoding="utf-8")
            if history is not None:
                history_path.write_text(json.dumps(history), encoding="utf-8")
            return subprocess.run(
                [
                    sys.executable,
                    str(VALIDATOR),
                    "--manifest",
                    str(manifest_path),
                    "--history",
                    str(history_path),
                ],
                cwd=ROOT,
                check=False,
                capture_output=True,
                text=True,
            )

    def valid_entry(self, **changes):
        entry = {
            "test_id": "backend::flaky_test",
            "owner": "@arena-owner",
            "issue": "#280",
            "failure_evidence": "consistent CI-only timeout",
            "scope": "backend unit test",
            "expires_on": "2099-12-31",
            "command": [sys.executable, "-c", "raise SystemExit(0)"],
            "passes_to_clear": 3,
        }
        entry.update(changes)
        return {"quarantines": [entry]}

    def test_ownerless_entry_fails(self):
        result = self.run_validator(self.valid_entry(owner=" "))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("owner must be a non-empty string", result.stderr)

    def test_expired_entry_fails(self):
        result = self.run_validator(self.valid_entry(expires_on="2020-01-01"))
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("quarantine expired", result.stderr)

    def test_passing_quarantine_must_reach_configured_streak(self):
        manifest = self.valid_entry(passes_to_clear=2)
        first = self.run_validator(manifest)
        self.assertEqual(first.returncode, 0)

        second = self.run_validator(manifest, history={"backend::flaky_test": 1})
        self.assertNotEqual(second.returncode, 0)
        self.assertIn("passed 2 consecutive runs", second.stderr)

    def test_isolated_failure_is_reported_as_disabled_coverage(self):
        manifest = self.valid_entry(
            command=[sys.executable, "-c", "raise SystemExit(7)"]
        )
        result = self.run_validator(manifest)
        self.assertEqual(result.returncode, 0)
        self.assertIn("DISABLED COVERAGE", result.stdout)


if __name__ == "__main__":
    unittest.main()
