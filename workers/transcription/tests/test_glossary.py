from __future__ import annotations

import pytest

from junto_transcribe.glossary import LIBRARY_LIMIT, TOKEN_LIMIT, counted_text, glossary_text, plan_glossary
from tests.conftest import Bench
from tests.fakes import signal


def words(text: str) -> int:
    return len(text.split())


def test_the_limit_is_the_recommended_one_and_below_what_the_library_keeps() -> None:
    assert TOKEN_LIMIT == 200
    assert LIBRARY_LIMIT == 448 // 2 - 1
    assert TOKEN_LIMIT < LIBRARY_LIMIT


def test_tokens_are_counted_on_the_text_as_the_library_tokenises_hotwords() -> None:
    text = glossary_text(["Oswin Thackerby", "Quorvex"])
    assert text == "Oswin Thackerby, Quorvex."
    assert counted_text(text) == " " + text.strip()


def test_an_empty_glossary_gives_no_prompt() -> None:
    plan = plan_glossary([], words)
    assert plan.text is None
    assert (plan.used, plan.cut, plan.token_count) == ((), (), 0)


def test_a_glossary_under_the_limit_is_used_whole() -> None:
    plan = plan_glossary(["Oswin Thackerby", "Quorvex"], words, token_limit=10)
    assert plan.text == "Oswin Thackerby, Quorvex."
    assert plan.used == ("Oswin Thackerby", "Quorvex")
    assert plan.cut == ()
    assert plan.token_count == 3


def test_terms_are_cut_from_the_end_of_the_priority_order() -> None:
    terms = ["Ysolde Varkonen", "Lumitrax", "Tarnbook", "Velkira"]
    plan = plan_glossary(terms, words, token_limit=3)
    assert plan.used == ("Ysolde Varkonen", "Lumitrax")
    assert plan.cut == ("Tarnbook", "Velkira")
    assert plan.token_count == 3
    assert plan.token_limit == 3


def test_a_later_term_that_would_fit_is_not_moved_ahead() -> None:
    plan = plan_glossary(["Zyphra", "Jurandira Pindobaçu Evaristo Castelhão", "Nexavio"], words, token_limit=2)
    assert plan.used == ("Zyphra",)
    assert plan.cut == ("Jurandira Pindobaçu Evaristo Castelhão", "Nexavio")


def test_a_first_term_over_the_limit_cuts_everything() -> None:
    plan = plan_glossary(["Brannoch Illesku"], words, token_limit=1)
    assert plan.text is None
    assert plan.used == ()
    assert plan.cut == ("Brannoch Illesku",)


def test_tokens_are_counted_on_the_whole_prompt() -> None:
    seen: list[str] = []

    def count(text: str) -> int:
        seen.append(text)
        return 1

    plan_glossary(["Junto", "Quorvex"], count)
    assert seen == [" Junto.", " Junto, Quorvex."]


def test_a_term_with_the_written_form_of_a_control_token_is_refused_when_the_job_is_read() -> None:
    from junto_transcribe.contract import JobError, parse_job
    from tests.conftest import job_document

    segments = [("m", "m.ogg", "microphone", "p", 0)]
    for index, term in enumerate(["<|en|>", "Quorvex<|translate|>", "<|0.00|> Lumitrax", "a |> b", "x <| y"]):
        glossary = ["Tarnbook"] * 0 + [f"Term {number}" for number in range(index)] + [term]
        with pytest.raises(JobError) as refused:
            parse_job(job_document(segments, glossary=glossary))
        assert (refused.value.code, refused.value.location) == ("glossary_control_token", f"/glossary/{index}")
        assert term not in str(refused.value)
    accepted = ["A<B", "x|y", "C++ <template>", "Zyphra (v2) > Nexavio", "|", "<", ">"]
    assert parse_job(job_document(segments, glossary=accepted)).glossary == tuple(accepted)


def test_a_term_that_the_tokenizer_turns_into_a_control_token_refuses_the_job(bench: Bench) -> None:
    from junto_transcribe.contract import JobError

    bench.add("a.ogg", signal(5, [(1.0, 2.0, 1)]))
    with pytest.raises(JobError) as refused:
        bench.run([("a", "a.ogg", "microphone", "a", 0)], glossary=["Quorvex", "Lumitrax", "Velkira [CTRL]"])
    assert (refused.value.code, refused.value.location) == ("glossary_control_token", "/glossary/2")
    assert "CTRL" not in str(refused.value)
    assert bench.recognizer.calls == []
    assert bench.files.opened == []
