"""The glossary as the text that Whisper reads before each clip."""

from __future__ import annotations

from collections.abc import Callable, Sequence
from dataclasses import dataclass

# faster-whisper keeps the first `448 // 2 - 1` tokens of its `hotwords` text and drops the rest
# without a message. The worker cuts earlier, at the limit that the technical test recommends,
# so the library's cut is never reached.
LIBRARY_LIMIT = 223
TOKEN_LIMIT = 200


@dataclass(frozen=True)
class GlossaryPlan:
    text: str | None
    used: tuple[str, ...]
    cut: tuple[str, ...]
    token_count: int
    token_limit: int


def glossary_text(terms: Sequence[str]) -> str:
    return ", ".join(terms) + "."


def counted_text(text: str) -> str:
    """The text as faster-whisper tokenises `hotwords`: stripped, after one space."""
    return " " + text.strip()


def plan_glossary(
    terms: Sequence[str],
    count_tokens: Callable[[str], int],
    token_limit: int = TOKEN_LIMIT,
) -> GlossaryPlan:
    """Keeps the longest prefix of `terms` whose text fits in `token_limit` tokens."""
    kept = 0
    token_count = 0
    for size in range(1, len(terms) + 1):
        tokens = count_tokens(counted_text(glossary_text(terms[:size])))
        if tokens > token_limit:
            break
        kept = size
        token_count = tokens
    used = tuple(terms[:kept])
    return GlossaryPlan(
        text=glossary_text(used) if used else None,
        used=used,
        cut=tuple(terms[kept:]),
        token_count=token_count,
        token_limit=token_limit,
    )
