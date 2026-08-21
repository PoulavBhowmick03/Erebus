"""Controls for the linkage measurement harness.

The harness exists to produce a number a wire change must move. That is only worth anything
if the number can move: a metric wired so it always reports 1.0 would record the leak and the
fix identically. Every test here is therefore a pair — the leak scores high, and a synthetic
fixed version of the same input scores at chance.

Not yet line-reviewed.
"""

from __future__ import annotations

import importlib.util
from pathlib import Path
import random
import sys
import unittest


ROOT = Path(__file__).resolve().parents[2]
LINKAGE_PATH = ROOT / "scripts" / "linkage.py"
SPEC = importlib.util.spec_from_file_location("erebus_linkage", LINKAGE_PATH)
if SPEC is None or SPEC.loader is None:  # pragma: no cover - import machinery failure
    raise RuntimeError(f"cannot load linkage module from {LINKAGE_PATH}")
linkage = importlib.util.module_from_spec(SPEC)
sys.modules[SPEC.name] = linkage
SPEC.loader.exec_module(linkage)


class M1TrafficClassification(unittest.TestCase):
    def test_the_current_wire_is_perfectly_separable(self) -> None:
        """The baseline this harness exists to record: F31 as a number."""
        rng = random.Random(0)
        positives = linkage.load_positives(
            ROOT / "scripts" / "fixtures", linkage.DEFAULT_FIXTURES
        )
        negatives = [linkage.synthetic_negative(rng, 5) for _ in range(5_000)]

        score = linkage.score_m1(positives, negatives)

        self.assertEqual(score.balanced_accuracy, 1.0)
        self.assertEqual(score.false_positive, 0)

    def test_padded_salts_score_at_chance(self) -> None:
        """The metric can move.

        Stands in for the Phase 8 fix by feeding the classifier salts whose spare bits are
        random rather than zero. If this still scored 1.0, the harness would be measuring
        something other than the fingerprint and could never show the fix working.
        """
        rng = random.Random(1)
        padded = [linkage.synthetic_negative(rng, 5) for _ in range(2_000)]
        negatives = [linkage.synthetic_negative(rng, 5) for _ in range(2_000)]

        score = linkage.score_m1(padded, negatives)

        self.assertAlmostEqual(score.balanced_accuracy, 0.5, places=3)

    def test_a_random_salt_essentially_never_has_the_shape(self) -> None:
        """The false-positive rate is a property of the salt space, not of the sample."""
        rng = random.Random(2)
        hits = sum(
            1 for _ in range(200_000) if linkage.has_v2_fifth_salt_shape(linkage.random_salt(rng))
        )
        self.assertEqual(hits, 0)


class M2ChangeBit(unittest.TestCase):
    def test_note_count_reveals_the_change_bit(self) -> None:
        rng = random.Random(3)
        labelled = linkage.synthetic_settlements(rng, 5_000)

        self.assertEqual(linkage.score_m2(labelled), 1.0)

    def test_a_constant_note_count_reduces_it_to_chance(self) -> None:
        """The Phase 8 fix is an always-minted change note, zero-valued when unneeded.

        Modelled by labelling both shapes with a constant count: the observer then sees the
        same seven notes either way and cannot do better than guessing.
        """
        rng = random.Random(4)
        constant = [
            (linkage.synthetic_negative(rng, linkage.CHANGE_SETTLEMENT_NOTES), truth)
            for _, truth in linkage.synthetic_settlements(rng, 5_000)
        ]

        accuracy = linkage.score_m2(constant)

        # Every guess is "change minted", so it is right exactly on the half that did.
        self.assertGreater(accuracy, 0.45)
        self.assertLess(accuracy, 0.55)

    def test_a_non_settlement_gets_no_answer_rather_than_a_guess(self) -> None:
        """Scoring a guess on an unreadable input would inflate every later measurement."""
        self.assertIsNone(linkage.change_minted((1, 2, 3)))


class ScoreArithmetic(unittest.TestCase):
    def test_an_empty_corpus_does_not_divide_by_zero(self) -> None:
        score = linkage.BinaryScore(0, 0, 0, 0)
        self.assertEqual(score.precision, 0.0)
        self.assertEqual(score.recall, 0.0)
        self.assertEqual(score.balanced_accuracy, 0.0)

    def test_balanced_accuracy_is_the_mean_of_the_two_rates(self) -> None:
        score = linkage.BinaryScore(
            true_positive=8, false_negative=2, true_negative=6, false_positive=4
        )
        self.assertAlmostEqual(score.recall, 0.8)
        self.assertAlmostEqual(score.true_negative_rate, 0.6)
        self.assertAlmostEqual(score.balanced_accuracy, 0.7)


if __name__ == "__main__":
    unittest.main()
