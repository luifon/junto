# Clicks one button of a system prompt, only when the prompt contains the expected text.
# Usage: python3 click_prompt.py <pid of the process that shows the prompt> <expected text> <button>
# JUNTO_TEST_AUTOMATION_CLI is the command of the desktop automation tool.
# The tool often reports an error for the click because the prompt closes before it can read it back;
# check the result of the action, not the reported status.
import json, os, shlex, subprocess, sys

pid, expected_text, button = sys.argv[1], sys.argv[2], sys.argv[3]
AUTOMATION = shlex.split(os.environ.get("JUNTO_TEST_AUTOMATION_CLI", ""))
if not AUTOMATION:
    sys.exit("set JUNTO_TEST_AUTOMATION_CLI to the command of the desktop automation tool")

def automation(*arguments):
    completed = subprocess.run([*AUTOMATION, *arguments, "--json"], capture_output=True, text=True)
    return json.loads(completed.stdout)

state = automation("get-app-state", "--app", f"pid:{pid}", "--no-screenshot")
if not state.get("ok"):
    print("STATE_ERROR", json.dumps(state.get("error")))
    sys.exit(1)
tree = state["result"]["snapshot"]["treeText"]
print(tree)
if expected_text not in tree:
    print("NOT_CLICKED: expected text is not in the dialog")
    sys.exit(1)
index = None
for line in tree.splitlines():
    parts = line.strip().split(" ", 2)
    if len(parts) == 3 and parts[1] == "button" and parts[2] == button:
        index = parts[0]
if index is None:
    print("NOT_CLICKED: button not found")
    sys.exit(1)
clicked = automation("click", "--app", f"pid:{pid}", "--element-index", index, "--no-screenshot")
print("CLICK", clicked.get("ok"), json.dumps(clicked.get("error") or clicked["result"].get("action") or clicked["result"].get("verification") or "")[:400])
