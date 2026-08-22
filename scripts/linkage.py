#!/usr/bin/env python3
"""Measure what an observer achieves against Erebus, as numbers rather than verdicts.

``observer.py`` answers "did this transaction leak?" for one transaction. This answers "how
well does an observer do across a corpus?", which is what a privacy claim needs before it
can be defended. The metrics are M1 and M2 in ``docs/threat-model.md``.

    python3 scripts/linkage.py
    python3 scripts/linkage.py --negatives 100000 --json

**M1 — traffic classification.** Can the historical fifth-salt classifier separate Erebus
records from ordinary pool traffic without any key? Wire-v3 positives are committed codec
outputs; historical v1/v2 fixtures remain negative controls. Negatives are synthetic
transactions of uniformly random in-range salts.

**M2 — the change bit.** Can an observer tell whether the payer had change? Wire v3 always
writes five data notes, one payment note, and one payer-owned change note. The change note is
zero for an exact payment. Note count no longer answers the question.

**What the negatives are, and are not.** Uniformly random salts are the correct *null model*
for M1: F31's claim is precisely that an Erebus salt does not look like an ordinary random
one. They are not a sample of real STRK20 traffic, so this measures distinguishability from
random rather than from the live pool. A corpus of real non-Erebus pool transactions would be
strictly better and needs chain access this script deliberately does not have. Recorded as a
limit in the report rather than hidden.

**Baseline.** The threat model requires a timing-only baseline: what an observer achieves
from timing and ordering alone, before any wire signal. That needs a real corpus and is not
implemented here; the report says so rather than reporting a baseline it did not measure.

Run this before and after a wire change. A privacy improvement that does not move a number
here has not been demonstrated.
"""

from __future__ import annotations

import argparse
import dataclasses
import json
from pathlib import Path
import random
import sys
from typing import Sequence

sys.path.insert(0, str(Path(__file__).parent))

from observer import (  # noqa: E402
    FLAG_BIT,
    SALT_LIMIT,
    ObserverError,
    has_v2_fifth_salt_shape,
    load_fixture,
    salts_from_calldata,
)

#: Wire v3 always creates five data notes, one payment note, and one change note.
SETTLEMENT_NOTES = 7

#: Codec-derived wire-v3 positive. This proves a codec property, not live system behavior.
DEFAULT_FIXTURES = ("observer-wire-v3.json",)

#: Historical transactions/fixtures that retain the classifier as a negative control. Both
#: are wire-v2-shaped for M1 purposes: the
#: fingerprint is a property of the fifth salt, which v1 also exhibits, and that is itself the
#: finding recorded in privacy-observer-finding.md.
HISTORICAL_FIXTURES = ("observer-wire-v1.json", "observer-wire-v2.json")


@dataclasses.dataclass(frozen=True)
class BinaryScore:
    """Counts and rates for one binary classifier over a labelled corpus."""

    true_positive: int
    false_positive: int
    true_negative: int
    false_negative: int

    @property
    def precision(self) -> float:
        """Of the transactions called Erebus, the fraction that were."""
        called = self.true_positive + self.false_positive
        return self.true_positive / called if called else 0.0

    @property
    def recall(self) -> float:
        """Of the Erebus transactions, the fraction found."""
        actual = self.true_positive + self.false_negative
        return self.true_positive / actual if actual else 0.0

    @property
    def true_negative_rate(self) -> float:
        """Of the non-Erebus transactions, the fraction correctly left alone."""
        actual = self.true_negative + self.false_positive
        return self.true_negative / actual if actual else 0.0

    @property
    def balanced_accuracy(self) -> float:
        """Mean of recall and true-negative rate.

        Reported instead of AUC on purpose. The classifier is a deterministic predicate with
        no score to threshold, so it has one operating point; an "AUC" over a single point is
        that point's balanced accuracy under a different name. 0.5 is chance, 1.0 is perfect
        separation.
        """
        return (self.recall + self.true_negative_rate) / 2


def random_salt(rng: random.Random) -> int:
    """A uniformly random salt in the contract's accepted range, ``2 <= salt < 2**120``."""
    return rng.randrange(2, SALT_LIMIT)


def synthetic_negative(rng: random.Random, notes: int) -> tuple[int, ...]:
    """One non-Erebus transaction: ``notes`` uniformly random in-range salts."""
    return tuple(random_salt(rng) for _ in range(notes))


def classify_erebus(salts: Sequence[int]) -> bool:
    """M1: does any salt carry the wire-v2 fifth-slot shape?"""
    return any(has_v2_fifth_salt_shape(salt) for salt in salts)


def change_minted(salts: Sequence[int]) -> bool | None:
    """M2: did this settlement mint a change note?

    Returns ``None`` when the note count matches neither settlement shape, which is the
    honest answer for a transaction that is not a settlement. A classifier that guessed on
    those would be scoring itself on inputs it cannot read.
    """
    if len(salts) == SETTLEMENT_NOTES:
        return True
    return None


def score_m1(
    positives: Sequence[Sequence[int]], negatives: Sequence[Sequence[int]]
) -> BinaryScore:
    """Run the M1 classifier over a labelled corpus."""
    true_positive = sum(1 for salts in positives if classify_erebus(salts))
    false_positive = sum(1 for salts in negatives if classify_erebus(salts))
    return BinaryScore(
        true_positive=true_positive,
        false_positive=false_positive,
        true_negative=len(negatives) - false_positive,
        false_negative=len(positives) - true_positive,
    )


def score_m2(labelled: Sequence[tuple[Sequence[int], bool]]) -> float:
    """M2 accuracy over settlements labelled with whether change was actually minted."""
    if not labelled:
        return 0.0
    correct = sum(1 for salts, truth in labelled if change_minted(salts) is truth)
    return correct / len(labelled)


def synthetic_settlements(
    rng: random.Random, count: int
) -> list[tuple[tuple[int, ...], bool]]:
    """Settlement-shaped transactions labelled with the ground truth M2 tries to recover.

    The salts are random because M2 reads note *count*, never content. Using random salts
    here keeps the measurement honest: it cannot accidentally succeed by reading a value.
    """
    out: list[tuple[tuple[int, ...], bool]] = []
    for _ in range(count):
        with_change = rng.random() < 0.5
        out.append((synthetic_negative(rng, SETTLEMENT_NOTES), with_change))
    return out


def load_positives(fixture_dir: Path, names: Sequence[str]) -> list[tuple[int, ...]]:
    """Public salts from each committed fixture."""
    positives: list[tuple[int, ...]] = []
    for name in names:
        salts = salts_from_calldata(load_fixture(fixture_dir / name))
        if not salts:
            raise ObserverError(f"fixture {name} contains no public salts")
        positives.append(salts)
    return positives


def _report(
    m1: BinaryScore, m2_accuracy: float, negatives: int, settlements: int
) -> dict[str, object]:
    return {
        "m1_traffic_classification": {
            "precision": m1.precision,
            "recall": m1.recall,
            "true_negative_rate": m1.true_negative_rate,
            "balanced_accuracy": m1.balanced_accuracy,
            "true_positive": m1.true_positive,
            "false_positive": m1.false_positive,
            "true_negative": m1.true_negative,
            "false_negative": m1.false_negative,
            "target": 0.5,
        },
        "m2_change_bit": {
            "accuracy": m2_accuracy,
            "settlements": settlements,
            "target": 0.5,
        },
        "corpus": {
            "synthetic_negatives": negatives,
            "negative_model": "uniform random salts in [2, 2**120)",
            "timing_only_baseline": None,
        },
        "limits": [
            "The wire-v3 positive is codec-derived, not a live Starknet transaction.",
            "Negatives are synthetic, not sampled from live STRK20 traffic.",
            "No timing-only baseline: it needs a real corpus this script does not fetch.",
            "M2 measures note count only; value-bearing ciphertext remains opaque.",
        ],
    }


def _print(report: dict[str, object]) -> None:
    m1 = report["m1_traffic_classification"]
    m2 = report["m2_change_bit"]
    corpus = report["corpus"]
    assert isinstance(m1, dict) and isinstance(m2, dict) and isinstance(corpus, dict)

    print("M1  traffic classification, no key")
    print(f"  precision            {m1['precision']:.4f}")
    print(f"  recall               {m1['recall']:.4f}")
    print(f"  true negative rate   {m1['true_negative_rate']:.4f}")
    print(f"  balanced accuracy    {m1['balanced_accuracy']:.4f}   target {m1['target']}")
    print(
        f"  counts               tp={m1['true_positive']} fp={m1['false_positive']} "
        f"tn={m1['true_negative']} fn={m1['false_negative']}"
    )
    print()
    print("M2  change bit from note count")
    print(f"  accuracy             {m2['accuracy']:.4f}   target {m2['target']}")
    print(f"  settlements          {m2['settlements']}")
    print()
    print(f"corpus                 {corpus['synthetic_negatives']} synthetic negatives")
    print(f"  negative model       {corpus['negative_model']}")
    print("  timing-only baseline not measured")
    print()
    print("limits")
    for limit in report["limits"]:  # type: ignore[union-attr]
        print(f"  - {limit}")


def _parser() -> argparse.ArgumentParser:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--negatives", type=int, default=10_000, help="synthetic non-Erebus transactions"
    )
    parser.add_argument(
        "--settlements", type=int, default=10_000, help="synthetic settlements for M2"
    )
    parser.add_argument("--seed", type=int, default=0, help="deterministic corpus seed")
    parser.add_argument("--json", action="store_true", help="emit the report as JSON")
    return parser


def main(argv: Sequence[str] | None = None) -> int:
    args = _parser().parse_args(argv)
    rng = random.Random(args.seed)
    fixture_dir = Path(__file__).parent / "fixtures"

    try:
        positives = load_positives(fixture_dir, DEFAULT_FIXTURES)
    except ObserverError as error:
        print(f"error: {error}", file=sys.stderr)
        return 2

    # Negatives carry the same note counts as the positives, so note count cannot leak the
    # label into M1. Without this the classifier could score well on a difference in size.
    note_counts = [len(salts) for salts in positives]
    negatives = [
        synthetic_negative(rng, note_counts[index % len(note_counts)])
        for index in range(args.negatives)
    ]

    m1 = score_m1(positives, negatives)
    settlements = synthetic_settlements(rng, args.settlements)
    m2_accuracy = score_m2(settlements)

    report = _report(m1, m2_accuracy, len(negatives), len(settlements))
    if args.json:
        print(json.dumps(report, indent=2))
    else:
        _print(report)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
