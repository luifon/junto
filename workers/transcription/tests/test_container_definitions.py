"""The image definition builds the worker by default, and both services have resource limits."""

from __future__ import annotations

import re
from pathlib import Path

import yaml

ROOT = Path(__file__).parent.parent
DOCKERFILE = (ROOT / "docker" / "Dockerfile").read_text("utf-8")
COMPOSE = yaml.safe_load((ROOT / "docker" / "compose.yaml").read_text("utf-8"))


def stages() -> list[tuple[str, str, str]]:
    """(name, base, body) of every stage, in file order."""
    parts = re.split(r"^FROM (\S+)(?: AS (\S+))?$", DOCKERFILE, flags=re.MULTILINE)
    return [(parts[index + 1] or "", parts[index], parts[index + 2]) for index in range(1, len(parts), 3)]


def stage(name: str) -> tuple[str, str]:
    return next((base, body) for found, base, body in stages() if found == name)


def test_a_build_without_a_target_gives_the_worker_image() -> None:
    name, base, body = stages()[-1]
    assert name == "worker"
    runtime_body = stage(base)[1] if base != "${PYTHON_IMAGE}" else ""
    assert 'ENTRYPOINT ["junto-transcribe"]' in body + runtime_body
    assert "pytest" not in body


def test_the_worker_image_has_no_development_dependency_and_no_test() -> None:
    base, body = stage("worker")
    text = body + (stage(base)[1] if base != "${PYTHON_IMAGE}" else "")
    assert "COPY --from=build /app/.venv /app/.venv" in text
    assert "build-test" not in text
    assert "COPY tests" not in text
    assert "USER junto" in text


def test_the_test_image_is_built_only_when_it_is_asked_for() -> None:
    names = [name for name, _, _ in stages()]
    assert "test" in names and names.index("test") < names.index("worker")
    for script in (ROOT / "tests" / "acceptance").glob("*.sh"):
        for line in script.read_text("utf-8").splitlines():
            if "docker build" in line and "docker/Dockerfile" in line:
                assert "--target" in line, f"{script.name}: {line.strip()}"


def test_the_build_commands_in_the_readme_give_the_worker_image() -> None:
    readme = (ROOT / "README.md").read_text("utf-8")
    commands = [line for line in readme.splitlines() if line.startswith("docker build")]
    assert len(commands) >= 2
    for command in commands:
        assert "--target" not in command or "--target worker" in command


def test_both_services_run_the_worker_stage_with_limits_and_without_a_network() -> None:
    assert set(COMPOSE["services"]) == {"worker-cpu", "worker-gpu"}
    for name, service in COMPOSE["services"].items():
        assert service["build"]["target"] == "worker", name
        assert service["network_mode"] == "none"
        assert service["read_only"] is True
        assert service["user"] == "10001:10001"
        assert "${JUNTO_WORKER_MEMORY:-" in str(service["mem_limit"])
        assert "${JUNTO_WORKER_CPUS:-" in str(service["cpus"])
        assert service["pids_limit"] > 0
        assert any(str(volume).endswith(":/models:ro") for volume in service["volumes"])
