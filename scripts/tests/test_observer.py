"""Positive and negative controls for the public-calldata observer harness.

The v1 fixture must decode and the v2 fixture must not. A harness that always refuses
proves nothing, so the positive control is what makes the negative result meaningful.

Not yet line-reviewed.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import sys
import unittest
from unittest import mock


ROOT = Path(__file__).resolve().parents[2]
OBSERVER_PATH = ROOT / "scripts" / "observer.py"
SPEC = importlib.util.spec_from_file_location("erebus_observer", OBSERVER_PATH)
if SPEC is None or SPEC.loader is None:  # pragma: no cover - import machinery failure
    raise RuntimeError(f"cannot load observer module from {OBSERVER_PATH}")
observer = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = observer
SPEC.loader.exec_module(observer)


class ObserverControls(unittest.TestCase):
    def analyse(self, fixture: str):
        path = ROOT / "scripts" / "fixtures" / fixture
        return observer.analyse_calldata(observer.load_fixture(path))

    def test_recovery_succeeds_on_a_wire_v1_fixture(self) -> None:
        analysis = self.analyse("observer-wire-v1.json")
        self.assertTrue(analysis.content_recovered)
        self.assertEqual(analysis.classified_wire_version, "wire-v1")
        self.assertEqual(
            analysis.recovered,
            (
                observer.Transcript(
                    message_type="accept",
                    reply_to=0,
                    created_at=1_785_480_617,
                    amount=1_000_000_000_000_000_000,
                    deadline=1_785_566_954,
                    memo_hash=0x5678,
                ),
            ),
        )

    def test_recovery_fails_on_a_wire_v2_fixture(self) -> None:
        analysis = self.analyse("observer-wire-v2.json")
        self.assertFalse(analysis.content_recovered)
        self.assertEqual(analysis.recovered, ())
        self.assertEqual(analysis.classified_wire_version, "wire-v2")

    def test_wire_v2_is_classified_separately_by_shape(self) -> None:
        analysis = self.analyse("observer-wire-v2.json")
        self.assertTrue(analysis.classified_as_erebus)
        self.assertEqual(len(analysis.fingerprint_salts), 1)

    def test_wire_v1_recovery_takes_precedence_over_a_shape_collision(self) -> None:
        analysis = self.analyse("observer-wire-v1.json")
        self.assertTrue(analysis.classified_as_erebus)
        self.assertEqual(len(analysis.fingerprint_salts), 2)
        self.assertEqual(analysis.classified_wire_version, "wire-v1")

    def test_wire_v3_has_no_legacy_version_label(self) -> None:
        analysis = self.analyse("observer-wire-v3.json")
        self.assertFalse(analysis.content_recovered)
        self.assertFalse(analysis.classified_as_erebus)
        self.assertIsNone(analysis.classified_wire_version)

    def test_transaction_hash_input_fetches_public_calldata(self) -> None:
        fixture = ROOT / "scripts" / "fixtures" / "observer-wire-v1.json"
        response = mock.MagicMock()
        response.__enter__.return_value = response
        response.__exit__.return_value = None
        response.read.return_value = (
            '{"jsonrpc":"2.0","id":1,"result":'
            + fixture.read_text(encoding="utf-8")
            + "}"
        ).encode()
        with mock.patch.object(observer.urllib.request, "urlopen", return_value=response):
            calldata = observer.fetch_transaction("0x1234", "http://rpc.invalid")

        self.assertTrue(observer.analyse_calldata(calldata).content_recovered)


if __name__ == "__main__":
    unittest.main()
