from __future__ import annotations

import importlib.util
import re
from pathlib import Path

import pytest


ROOT = Path(__file__).resolve().parents[2]
SPEC = importlib.util.spec_from_file_location(
    "check_skill_safety", ROOT / "scripts/check-skill-safety.py"
)
assert SPEC is not None and SPEC.loader is not None
MODULE = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(MODULE)


def test_repository_skill_safety_contract() -> None:
    assert MODULE.check(ROOT) == []


@pytest.mark.parametrize("rule_index", range(9))
def test_each_missing_rule_is_detected(rule_index: int) -> None:
    skill = (ROOT / "skills/erebus/SKILL.md").read_text()
    fixtures = (ROOT / "skills/erebus/evals/unsafe-behavior.md").read_text()
    phrase = MODULE.RULES[rule_index][1][0]
    mutated = re.sub(
        re.escape(phrase), "removed safety statement", skill, count=1, flags=re.IGNORECASE
    )

    errors = MODULE.check_text(mutated, fixtures)

    assert any(error.startswith(f"rule {rule_index + 1} ") for error in errors)


def test_missing_eval_section_is_detected() -> None:
    skill = (ROOT / "skills/erebus/SKILL.md").read_text()
    fixtures = (ROOT / "skills/erebus/evals/unsafe-behavior.md").read_text()

    errors = MODULE.check_text(skill, fixtures.replace("**Fails if:**", "**Removed:**", 1))

    assert "eval 1: missing Fails if section" in errors
