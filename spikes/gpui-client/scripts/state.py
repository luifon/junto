import json, shutil, subprocess, sys

state_path, image_path, show_tree = sys.argv[1], sys.argv[2], sys.argv[3]
state = json.load(open(state_path))
if not state.get("ok"):
    print(json.dumps(state)[:800])
    sys.exit(1)
result = state["result"]
shot = result.get("screenshot") or {}
if shot.get("path"):
    shutil.copyfile(shot["path"], image_path)
    window = result.get("snapshot", {}).get("window", {})
    width = int(window.get("width") or 1400)
    subprocess.run(["sips", "--resampleWidth", str(width), image_path], capture_output=True)
    print("saved", image_path, "resampled to window width", width, "so image pixels equal window coordinates")
else:
    print("no screenshot:", result.get("screenshotStatus"))
action = result.get("action")
if action:
    print("action:", json.dumps(action)[:300])
if show_tree == "1":
    print(result.get("snapshot", {}).get("treeText", ""))
