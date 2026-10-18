#!/usr/bin/env python3
"""Content for the tests of the backup, the restore and the upgrade (ADR 0911).

usage: content_driver.py <step> --project <name> --env-file <file> --compose-file <file>
           --origin <http://127.0.0.1:port> --state <file> [--count <n>] [--stop-file <file>]
           [--backup <directory>] [--restore-log <file>] [--set <key>=<number>]

Steps of deploy/tests/backup-restore.sh:
  prepare   an owner, three channels with messages and attachments, three recordings; one
            channel and one recording are deleted, and their directories leave the disk
  write     sends messages with attachments to the kept channel until the stop file exists
  between   runs between the copy of the database and the list of files of a backup: deletes
            the second channel and the second recording, and waits longer than the server
            waits before it deletes their directories
  released  after the backup: those directories leave the disk
  verify    in the restored deployment: every message, attachment and recording directory
            that the database names is present, and nothing of what was deleted before; the
            restore listed the directories of the archive that the database does not name

Steps of deploy/tests/upgrade.sh:
  seed      an owner and one channel with messages and attachments
  send      one further message with an attachment
  check     the channel holds exactly --count messages of the test, each with its file

The driver talks to a running deployment through the API and through "docker compose". It
prints one line per check and ends with the code 1 when a check failed.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import os
import re
import subprocess
import sys
import time
import uuid
from pathlib import Path
from typing import Any, Callable

from junto_api import Device
from server_output import newest_owner_code, standard_output

FILES = "/var/lib/junto"
# The server deletes the directory of a removed attachment at the first maintenance pass
# that runs at least 60 seconds after the removal, and a pass runs every 60 seconds: 120
# seconds at most. The tests wait this long for a deletion, and this long to see that a
# backup holds a deletion back.
DELETION_SECONDS = 150


class Failed(Exception):
    """A step that the later steps need did not work."""


failures = 0


def check(text: str, holds: bool, detail: object = "") -> bool:
    global failures
    if holds:
        print(f"ok    {text}", flush=True)
    else:
        failures += 1
        print(f"FAIL  {text}" + (f": {detail}" if detail != "" else ""), flush=True)
    return holds


def wait_for(what: str, holds: Callable[[], Any], seconds: float, pause: float = 2) -> Any:
    deadline = time.monotonic() + seconds
    while True:
        value = holds()
        if value:
            return value
        if time.monotonic() > deadline:
            raise Failed(f"after {seconds:.0f} s this did not hold: {what}")
        time.sleep(pause)


class Deployment:
    def __init__(self, arguments: argparse.Namespace) -> None:
        self.directory = Path(__file__).resolve().parent.parent
        self.command = [
            "docker", "compose", "-p", arguments.project, "--env-file", arguments.env_file,
            "-f", arguments.compose_file,
        ]  # fmt: skip

    def run(self, *arguments: str, data: str | None = None) -> str:
        done = subprocess.run(
            [*self.command, *arguments],
            cwd=self.directory,
            capture_output=True,
            text=True,
            input=data,
            stdin=None if data is not None else subprocess.DEVNULL,
            timeout=300,
            check=False,
        )
        if done.returncode != 0:
            raise Failed(f"docker compose {' '.join(arguments[:3])} ended with {done.returncode}: {done.stderr[-400:]}")
        return done.stdout

    def inside(self, script: str, data: str | None = None) -> str:
        return self.run("exec", "-T", "server", "/bin/sh", "-c", script, data=data)

    def sql(self, query: str) -> list[str]:
        out = self.run("exec", "-T", "postgres", "psql", "-U", "junto", "-d", "junto", "-At", "-v", "ON_ERROR_STOP=1", "-c", query)
        return [line for line in out.splitlines() if line]

    def owner_code(self) -> str:
        code = newest_owner_code(standard_output(self.command, self.directory, "server"))
        if code is None:
            raise Failed("the standard output of the server has no owner code")
        return code

    def present(self, directories: list[str]) -> list[str]:
        """The directories of the list, relative to the files of the server, that exist."""
        script = f'cd {FILES} && while IFS= read -r d; do [ -e "$d" ] && printf "%s\\n" "$d"; done; true'
        return self.inside(script, data="".join(f"{d}\n" for d in directories)).split()

    def checksums(self, directory: str) -> dict[str, str]:
        """The SHA-256 of each file below a directory of the files of the server."""
        out = self.inside(f"cd {FILES}/{directory} && find . -type f -exec sha256sum {{}} +")
        return {name: digest for digest, name in (line.split(None, 1) for line in out.splitlines())}


class State:
    """What one step tells the later steps, as a JSON file."""

    def __init__(self, path: str) -> None:
        self.path = Path(path)
        self.values: dict[str, Any] = json.loads(self.path.read_text()) if self.path.exists() else {}

    def save(self) -> None:
        self.path.write_text(json.dumps(self.values, indent=1))

    def __getitem__(self, key: str) -> Any:
        return self.values[key]

    def __setitem__(self, key: str, value: Any) -> None:
        self.values[key] = value


def owner_of(arguments: argparse.Namespace, state: State) -> Device:
    device = Device(arguments.origin, bytes.fromhex(state["owner_secret"]))
    device.login()
    return device


def send(owner: Device, channel_id: str, text: str, size: int = 20_000) -> dict[str, str]:
    """One message with one attachment of random bytes. Returns what the server stored."""
    content = os.urandom(size)
    digest = hashlib.sha256(content).hexdigest()
    upload = owner.call(
        "POST",
        "/api/v1/attachments",
        {"channel_id": channel_id, "name": f"{text}.bin", "bytes": size, "sha256": digest},
        expect=201,
    ).json()
    owner.call("PUT", f"/api/v1/attachments/{upload['id']}/chunks/0", content=content, expect=200)
    owner.call("POST", f"/api/v1/attachments/{upload['id']}/complete", expect=200)
    document = {"blocks": [{"kind": "paragraph", "content": [{"kind": "text", "text": text}]}]}
    message = owner.call(
        "POST", f"/api/v1/channels/{channel_id}/messages", {"document": document, "attachments": [upload["id"]]}, expect=201
    ).json()
    return {"message": message["id"], "attachment": upload["id"], "sha256": digest, "text": text}


def messages_of(owner: Device, channel_id: str) -> list[dict[str, Any]]:
    found: list[dict[str, Any]] = []
    after = 0
    while True:
        page = owner.get(f"/api/v1/channels/{channel_id}/messages?after={after}&limit=200")
        found += page["messages"]
        if not page["has_more"] or not page["messages"]:
            return found
        after = page["messages"][-1]["position"]


def files_match(owner: Device, messages: list[dict[str, Any]]) -> list[str]:
    """The messages that do not have exactly one attachment, as every message of these
    tests has, or whose attachment the server does not return with the stated bytes."""
    wrong = [message["id"] for message in messages if len(message["attachments"]) != 1]
    for message in messages:
        for attachment in message["attachments"]:
            answer = owner.call("GET", f"/api/v1/attachments/{attachment['id']}/file")
            if answer.status != 200 or hashlib.sha256(answer.body).hexdigest() != attachment["sha256"]:
                wrong.append(message["id"])
    return wrong


def text_of(message: dict[str, Any]) -> str:
    try:
        return str(message["document"]["blocks"][0]["content"][0]["text"])
    except (KeyError, IndexError, TypeError):
        return ""


def new_owner(arguments: argparse.Namespace, deployment: Deployment, state: State) -> tuple[Device, str, str]:
    owner = Device(arguments.origin)
    owner.claim_owner(deployment.owner_code(), "Owner")
    state["owner_secret"] = owner.secret.hex()
    snapshot = owner.get("/api/v1/snapshot")
    workspace = snapshot["workspaces"][0]
    workspace_id = workspace["workspace"]["id"]
    voice = next(channel["id"] for channel in workspace["channels"] if channel["kind"] == "voice")
    return owner, workspace_id, voice


def new_channel(owner: Device, workspace_id: str, name: str) -> str:
    created = owner.call("POST", f"/api/v1/workspaces/{workspace_id}/channels", {"name": name, "kind": "text"}, expect=(200, 201))
    return str(created.json()["id"])


def stored_directories(deployment: Deployment, attachments: list[str]) -> list[str]:
    ids = ",".join(f"'{uuid.UUID(attachment)}'" for attachment in attachments)
    return [f"attachments/{line}" for line in deployment.sql(f"select distinct storage_id from attachments where id in ({ids})")]


def new_recording(deployment: Deployment, owner: Device, workspace_id: str, voice: str) -> dict[str, Any]:
    """A finished recording as rows of the database and two files. The recorder is not part
    of these tests (deploy/tests/recording.sh records a call); a backup copies the directory
    of a recording whatever wrote it."""
    recording = str(uuid.uuid4())
    room = f"jr-{os.urandom(6).hex()}-{os.urandom(16).hex()}"
    deployment.sql(
        "insert into recordings (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,"
        " requested_at, recording_since, stop_reason, stopped_by, stop_requested_at, ended_at, processed_at)"
        f" values ('{recording}', '{workspace_id}', '{voice}', '{uuid.uuid4()}', '{room}', 'ready', '{owner.account_id}',"
        f" 'auto', now() - interval '10 minutes', now() - interval '10 minutes', 'stopped', '{owner.account_id}',"
        " now() - interval '5 minutes', now() - interval '5 minutes', now() - interval '4 minutes')"
    )
    deployment.inside(
        f"mkdir {FILES}/recordings/{recording} && head -c 60000 /dev/urandom > {FILES}/recordings/{recording}/composite.mp4"
        f" && head -c 30000 /dev/urandom > {FILES}/recordings/{recording}/microphone.ogg"
    )
    return {"id": recording, "files": deployment.checksums(f"recordings/{recording}")}


def step_prepare(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner, workspace_id, voice = new_owner(arguments, deployment, state)
    channels = {name: new_channel(owner, workspace_id, name) for name in ("kept", "deleted-before", "deleted-between")}
    state["channels"] = channels
    sent: dict[str, list[dict[str, str]]] = {}
    for name, channel_id in channels.items():
        sent[name] = [send(owner, channel_id, f"{name}-{number}") for number in range(3)]
    sent["kept"].append(send(owner, channels["kept"], "kept-large", size=400_000))
    state["sent"] = sent
    state["directories"] = {
        name: stored_directories(deployment, [entry["attachment"] for entry in entries]) for name, entries in sent.items()
    }
    state["recordings"] = {
        name: new_recording(deployment, owner, workspace_id, voice) for name in ("kept", "removed-before", "removed-between")
    }
    # A recording that failed has a row and no directory.
    state["failed_recording"] = str(uuid.uuid4())
    deployment.sql(
        "insert into recordings (id, workspace_id, channel_id, call_id, room_name, state, started_by, language,"
        " requested_at, ended_at, failure)"
        f" values ('{state['failed_recording']}', '{workspace_id}', '{voice}', '{uuid.uuid4()}',"
        f" 'jr-{os.urandom(6).hex()}-{os.urandom(16).hex()}', 'failed', '{owner.account_id}', 'auto',"
        " now() - interval '10 minutes', now() - interval '9 minutes', 'nothing_recorded')"
    )
    state.save()

    every = [directory for directories in state["directories"].values() for directory in directories]
    every += [f"recordings/{recording['id']}" for recording in state["recordings"].values()]
    check(f"the {len(every)} directories of the attachments and of the recordings are on the disk", sorted(deployment.present(every)) == sorted(every))
    gone = [*state["directories"]["deleted-before"], f"recordings/{state['recordings']['removed-before']['id']}"]
    owner.call("DELETE", f"/api/v1/channels/{channels['deleted-before']}", expect=(200, 204))
    owner.call("DELETE", f"/api/v1/recordings/{state['recordings']['removed-before']['id']}", expect=204)
    started = time.monotonic()
    wait_for(
        "the server deleted the directories of the channel and of the recording that were deleted before the backup",
        lambda: not deployment.present(gone),
        DELETION_SECONDS,
    )
    check(f"the directories of what was deleted before the backup left the disk after {time.monotonic() - started:.0f} s", True)


def step_write(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner = owner_of(arguments, state)
    channel_id = state["channels"]["kept"]
    log = Path(arguments.stop_file + ".sent")
    number = 0
    with log.open("a") as written:
        while not Path(arguments.stop_file).exists():
            entry = send(owner, channel_id, f"written-{number}")
            entry["at"] = str(time.time())
            written.write(json.dumps(entry) + "\n")
            written.flush()
            number += 1
            time.sleep(0.5)


def step_between(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner = owner_of(arguments, state)
    held = [*state["directories"]["deleted-between"], f"recordings/{state['recordings']['removed-between']['id']}"]
    state["backups_running"] = deployment.sql("select count(*) from backups where expires_at > now()")[0]
    owner.call("DELETE", f"/api/v1/channels/{state['channels']['deleted-between']}", expect=(200, 204))
    owner.call("DELETE", f"/api/v1/recordings/{state['recordings']['removed-between']['id']}", expect=204)
    state["between_at"] = time.time()
    time.sleep(DELETION_SECONDS)
    state["held"] = deployment.present(held)
    state["held_expected"] = held
    state.save()


def step_released(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    held = state["held_expected"]
    check("the backup ran when the second channel and the second recording were deleted", state["backups_running"] == "1")
    check(
        f"{DELETION_SECONDS} s after their deletion, during the backup, their directories were on the disk",
        sorted(state["held"]) == sorted(held),
        state["held"],
    )
    check("no backup holds deletions back after the backup", deployment.sql("select count(*) from backups") == ["0"])
    listing = subprocess.run(["tar", "-tf", f"{arguments.backup}/files.tar"], capture_output=True, text=True, check=True)
    members = {name.rstrip("/") for name in listing.stdout.splitlines()}
    before = [*state["directories"]["deleted-before"], f"recordings/{state['recordings']['removed-before']['id']}"]
    kept = [*state["directories"]["kept"], f"recordings/{state['recordings']['kept']['id']}"]
    check("the archive holds the directories of the kept channel and recording", set(kept) <= members)
    check("the archive holds the directories of what was deleted after the copy of the database", set(held) <= members)
    check("the archive holds no directory of what was deleted before the backup", not set(before) & members)
    left_out = Path(arguments.backup, "left-out").read_text().splitlines()
    failed = [line for line in left_out if state["failed_recording"] in line]
    others = [line for line in left_out if not re.fullmatch(r"skip attachments/[0-9a-f-]{36} upload_not_complete", line)]
    check("the file left-out has no line for the recording that failed and has no file", failed == [], failed)
    check("its lines are uploads that had not ended", others == [], others[:3])
    wait_for("the server deleted those directories after the backup", lambda: not deployment.present(held), DELETION_SECONDS)
    check("after the backup the server deleted those directories", True)


def step_verify(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner = owner_of(arguments, state)
    channels, sent, recordings = state["channels"], state["sent"], state["recordings"]
    written = [json.loads(line) for line in Path(arguments.stop_file + ".sent").read_text().splitlines()]
    before_backup = [entry for entry in written if float(entry["at"]) < state["backup_started"]]

    # The channel that was deleted between the copy of the database and the list of files
    # exists in the restored database, with its files.
    for name in ("kept", "deleted-between"):
        messages = [message for message in messages_of(owner, channels[name]) if not message["deleted"] and message["document"]]
        texts = [text_of(message) for message in messages]
        expected = [entry["text"] for entry in sent[name]]
        check(f"channel {name}: the messages of the preparation are present", texts[: len(expected)] == expected, texts[:6])
        check(f"channel {name}: every attachment of its {len(messages)} messages has the stated bytes", files_match(owner, messages) == [])
        by_id = {entry["message"]: entry["sha256"] for entry in [*sent[name], *written]}
        differs = [m["id"] for m in messages if [a["sha256"] for a in m["attachments"]] != [by_id.get(m["id"])]]
        check(f"channel {name}: every message has the attachment that was sent with it", differs == [], differs[:3])
        if name == "kept":
            restored = texts[len(expected) :]
            check(
                "channel kept: the messages that were written during the backup are a first part of what was sent",
                restored == [entry["text"] for entry in written[: len(restored)]],
            )
            check(
                f"channel kept: all {len(before_backup)} messages from before the backup are present, and {len(restored)} of {len(written)} in all",
                len(restored) >= len(before_backup) > 0,
            )

    gone = owner.call("GET", f"/api/v1/channels/{channels['deleted-before']}/messages")
    check("the channel that was deleted before the backup is not returned", gone.status == 404, gone.status)
    listed = [channel["id"] for workspace in owner.get("/api/v1/snapshot")["workspaces"] for channel in workspace["channels"]]
    check("the snapshot holds the two channels of the backup", channels["kept"] in listed and channels["deleted-between"] in listed)
    check("the snapshot does not hold the channel that was deleted before", channels["deleted-before"] not in listed)

    before = [*state["directories"]["deleted-before"], f"recordings/{recordings['removed-before']['id']}"]
    check("no directory of what was deleted before the backup is on the disk", deployment.present(before) == [], deployment.present(before))
    for entry in sent["deleted-before"]:
        answer = owner.call("GET", f"/api/v1/attachments/{entry['attachment']}/file")
        if answer.status != 404:
            check("an attachment of the channel that was deleted before is not returned", False, answer.status)

    # Every directory that the restored database names.
    named = [f"attachments/{line}" for line in deployment.sql("select distinct storage_id from attachments where complete and removed_at is null")]
    absent = sorted(set(named) - set(deployment.present(named)))
    check(f"every one of the {len(named)} directories of complete attachments in the database is on the disk", absent == [] and len(named) > 0, absent[:3])
    failed = deployment.sql("select id from recordings where state = 'failed'")
    check("the database names the recording that failed", failed == [state["failed_recording"]], failed)
    rows = deployment.sql("select id from recordings where state in ('ready', 'partly_failed')")
    check(
        "the database names the kept recording and the recording that was removed during the backup",
        sorted(rows) == sorted([recordings["kept"]["id"], recordings["removed-between"]["id"]]),
        rows,
    )
    for name in ("kept", "removed-between"):
        found = deployment.checksums(f"recordings/{recordings[name]['id']}") if deployment.present([f"recordings/{recordings[name]['id']}"]) else {}
        check(f"recording {name}: its files have the bytes of the backup", found == recordings[name]["files"], sorted(found))

    # The directories of the archive that the restored database does not name: uploads that
    # ended between the copy of the database and the list of the files. The restore states
    # them as the server lists them.
    listing = subprocess.run(["tar", "-tf", f"{arguments.backup}/files.tar"], capture_output=True, text=True, check=True)
    archived = {"/".join(parts[:2]) for parts in (name.rstrip("/").split("/") for name in listing.stdout.splitlines()) if len(parts) >= 2}
    rows = {f"attachments/{line}" for line in deployment.sql("select distinct storage_id from attachments")}
    rows |= {f"recordings/{line}" for line in deployment.sql("select id::text from recordings union select split_part(path, '/', 1) from file_removals")}
    expected = sorted(archived - rows)
    stated = re.findall(r"^(?:unnamed|skip) ((?:attachments|recordings)/[0-9a-f-]{36})(?: changed_recently)?$", Path(arguments.restore_log).read_text(), re.M)
    check(
        f"the restore listed the {len(expected)} directories of the archive that the restored database does not name",
        sorted(stated) == expected and len(expected) > 0,
        (len(stated), len(expected)),
    )
    check("each of those directories is on the disk", sorted(deployment.present(expected)) == expected)

    # tar restores the time of the last change of a directory, so the 24 hours of the server
    # are counted from before the backup.
    kept = state["directories"]["kept"]
    times = deployment.inside(f"cd {FILES} && stat -c %Y " + " ".join(kept)).split()
    check(
        "a restored directory has the time of its last change from before the backup",
        len(times) == len(kept) and all(int(changed) < state["backup_started"] for changed in times),
        times,
    )

    check("the restored database holds no backup", deployment.sql("select count(*) from backups") == ["0"])
    check("the directory of the exports is empty", deployment.inside(f"ls -A {FILES}/exports").strip() == "")
    after = send(owner, channels["kept"], "after-the-restore")
    check("the restored server takes a new message with an attachment", bool(after["message"]))


def step_seed(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner, workspace_id, _ = new_owner(arguments, deployment, state)
    state["channel"] = new_channel(owner, workspace_id, "upgrade")
    state["sent"] = [send(owner, state["channel"], f"before-the-upgrade-{number}") for number in range(arguments.count)]
    state.save()
    check(f"{arguments.count} messages with attachments were stored", True)


def step_send(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner = owner_of(arguments, state)
    send(owner, state["channel"], f"after-the-upgrade-{int(time.time())}")
    check("a further message with an attachment was stored", True)


def step_check(arguments: argparse.Namespace, deployment: Deployment, state: State) -> None:
    owner = owner_of(arguments, state)
    messages = [message for message in messages_of(owner, state["channel"]) if message["document"]]
    texts = [text_of(message) for message in messages]
    expected = [entry["text"] for entry in state["sent"]]
    check(f"the channel holds the {len(expected)} messages from before the upgrade first", texts[: len(expected)] == expected, texts)
    check(f"the channel holds {arguments.count} messages of the test", len(texts) == arguments.count, texts)
    stated = [[a["sha256"] for a in message["attachments"]] for message in messages[: len(expected)]]
    check("each of those messages has the attachment that was sent with it", stated == [[entry["sha256"]] for entry in state["sent"]])
    check("every attachment has the stated bytes", files_match(owner, messages) == [])


STEPS = {
    "prepare": step_prepare,
    "write": step_write,
    "between": step_between,
    "released": step_released,
    "verify": step_verify,
    "seed": step_seed,
    "send": step_send,
    "check": step_check,
}


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("step", choices=sorted(STEPS))
    for name in ("project", "env-file", "compose-file", "origin", "state"):
        parser.add_argument(f"--{name}", required=True)
    parser.add_argument("--count", type=int, default=3)
    parser.add_argument("--stop-file", default="")
    parser.add_argument("--backup", default="")
    parser.add_argument("--restore-log", default="")
    parser.add_argument("--set", action="append", default=[], metavar="KEY=NUMBER")
    arguments = parser.parse_args()
    # A step saves the state when it changed it. "write" runs beside other steps and never
    # saves.
    state = State(arguments.state)
    if arguments.set:
        for pair in arguments.set:
            key, value = pair.split("=", 1)
            state[key] = float(value)
        state.save()
    try:
        STEPS[arguments.step](arguments, Deployment(arguments), state)
    except Failed as failed:
        print(f"FAIL  {failed}", flush=True)
        return 1
    return 1 if failures else 0


if __name__ == "__main__":
    sys.exit(main())
