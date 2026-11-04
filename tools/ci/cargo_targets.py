#!/usr/bin/env python3
"""Prints the options of `cargo test` that select every test target of an earlier build except
some (ADR 0905):

    cargo_targets.py <messages file> <excluded test target>...

<messages file> holds the JSON messages of `cargo test --workspace --no-run
--message-format=json-render-diagnostics`: the programs that `cargo test --workspace` builds and
runs. The output is `--lib --bins` and one `--test <name>` for each integration test target of
that build that is not excluded, so that `cargo test --workspace` with these options and one
command for each excluded target, together with `--doc`, run the same test programs as one
`cargo test --workspace`. With --workspace in every command the features of each package are
those of the earlier build, and Cargo builds nothing again.
"""
import json
import sys


def options(lines, excluded):
    names, seen_excluded = [], set()
    for line in lines:
        line = line.strip()
        if not line.startswith("{"):
            continue
        message = json.loads(line)
        if message.get("reason") != "compiler-artifact" or not message.get("profile", {}).get("test"):
            continue
        target = message["target"]
        if target["kind"] != ["test"]:
            continue
        if target["name"] in excluded:
            seen_excluded.add(target["name"])
        elif target["name"] not in names:
            names.append(target["name"])
    missing = sorted(set(excluded) - seen_excluded)
    if missing:
        raise ValueError(f"the build has no test target {', '.join(missing)}")
    if not names:
        raise ValueError("the build has no other test target")
    return ["--lib", "--bins", *[word for name in names for word in ("--test", name)]]


if __name__ == "__main__":
    if len(sys.argv) < 2:
        print("usage: cargo_targets.py <messages file> <excluded test target>...", file=sys.stderr)
        sys.exit(2)
    try:
        with open(sys.argv[1], encoding="utf-8") as file:
            print(" ".join(options(file, sys.argv[2:])))
    except (OSError, ValueError, KeyError, json.JSONDecodeError) as error:
        print(error, file=sys.stderr)
        sys.exit(2)
