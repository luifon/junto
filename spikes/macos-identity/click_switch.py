# Reads, and with "click" changes, the Screen Recording switch of one row in System Settings.
# Usage: python3 click_switch.py <row name> [click]
# JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
import json, os, shlex, subprocess, sys

row_name = sys.argv[1]
APP = "com.apple.systempreferences"
AUTOMATION = shlex.split(os.environ.get("JUNTO_TEST_AUTOMATION_CLI", ""))
if not AUTOMATION:
    sys.exit("set JUNTO_TEST_AUTOMATION_CLI to the command of the desktop automation tool")

def automation(*arguments):
    completed = subprocess.run([*AUTOMATION, *arguments, "--json"], capture_output=True, text=True)
    return json.loads(completed.stdout)

def switch_of(tree):
    lines = tree.splitlines()
    if "Window: \"Screen & System Audio Recording\"" not in tree:
        return None, None
    for position, line in enumerate(lines):
        if line.strip().split(" ", 1)[-1] == f"outline row {row_name}":
            for following in lines[position + 1: position + 4]:
                parts = following.strip().split(" ", 1)
                if len(parts) == 2 and parts[1].startswith("switch"):
                    return parts[0], parts[1]
    return None, None

state = automation("get-app-state", "--app", APP, "--no-screenshot")
index, value = switch_of(state["result"]["snapshot"]["treeText"]) if state.get("ok") else (None, None)
print("BEFORE", index, value)
if index is None:
    sys.exit(1)
if len(sys.argv) > 2 and sys.argv[2] == "click":
    clicked = automation("click", "--app", APP, "--element-index", index, "--no-screenshot")
    print("CLICK", clicked.get("ok"), json.dumps(clicked.get("error") or "")[:300])
    after = automation("get-app-state", "--app", APP, "--no-screenshot")
    if after.get("ok"):
        tree = after["result"]["snapshot"]["treeText"]
        print("AFTER", switch_of(tree))
        print(tree[:200])
    else:
        print("AFTER_ERROR", json.dumps(after.get("error"))[:300])
