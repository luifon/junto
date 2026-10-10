# usage: atspi_tool.py dump <pid> | press <pid> <name> | apps
# Reads the AT-SPI tree of one process through the Atspi introspection binding.
import sys

import gi

gi.require_version("Atspi", "2.0")
from gi.repository import Atspi

STATES = [
    ("focused", Atspi.StateType.FOCUSED),
    ("focusable", Atspi.StateType.FOCUSABLE),
    ("checked", Atspi.StateType.CHECKED),
    ("pressed", Atspi.StateType.PRESSED),
    ("editable", Atspi.StateType.EDITABLE),
    ("expanded", Atspi.StateType.EXPANDED),
    ("selected", Atspi.StateType.SELECTED),
    ("showing", Atspi.StateType.SHOWING),
]


def applications():
    desktop = Atspi.get_desktop(0)
    found = []
    for index in range(desktop.get_child_count()):
        try:
            child = desktop.get_child_at_index(index)
            if child is not None:
                found.append(child)
        except Exception:
            pass
    return found


def application(pid):
    for candidate in applications():
        try:
            if candidate.get_process_id() == pid:
                return candidate
        except Exception:
            pass
    return None


def describe(node):
    parts = [node.get_role_name()]
    name = node.get_name()
    if name:
        parts.append(repr(name))
    description = node.get_description()
    if description:
        parts.append("description=" + repr(description))
    states = node.get_state_set()
    flags = [label for label, state in STATES if states.contains(state)]
    if flags:
        parts.append("[" + " ".join(flags) + "]")
    try:
        actions = [node.get_action_name(i) for i in range(node.get_n_actions())]
        if actions:
            parts.append("actions=" + ",".join(actions))
    except Exception:
        pass
    try:
        text = node.get_text_iface()
        if text is not None:
            content = Atspi.Text.get_text(text, 0, min(Atspi.Text.get_character_count(text), 80))
            parts.append("text=" + repr(content))
    except Exception:
        pass
    try:
        value = node.get_value_iface()
        if value is not None:
            parts.append("value=%s" % Atspi.Value.get_current_value(value))
    except Exception:
        pass
    return " ".join(parts)


def walk(node, depth, visit):
    visit(node, depth)
    try:
        count = node.get_child_count()
    except Exception:
        return
    for index in range(count):
        try:
            child = node.get_child_at_index(index)
        except Exception:
            continue
        if child is not None:
            walk(child, depth + 1, visit)


def main():
    command = sys.argv[1]
    if command == "apps":
        for candidate in applications():
            print(candidate.get_process_id(), repr(candidate.get_name()), candidate.get_toolkit_name())
        return 0
    pid = int(sys.argv[2])
    root = application(pid)
    if root is None:
        print("no AT-SPI application for pid", pid)
        return 1
    if command == "dump":
        counter = [0]

        def show(node, depth):
            counter[0] += 1
            print("  " * depth + describe(node))

        print("toolkit:", root.get_toolkit_name(), root.get_toolkit_version())
        walk(root, 0, show)
        print("nodes:", counter[0])
        return 0
    if command == "press":
        wanted = sys.argv[3]
        hits = []
        walk(root, 0, lambda node, depth: hits.append(node) if node.get_name() == wanted else None)
        for node in hits:
            try:
                if node.get_n_actions() > 0:
                    print("press", repr(wanted), "->", node.do_action(0))
                    return 0
            except Exception as error:
                print("press failed:", error)
        print("no element with an action named", repr(wanted))
        return 1
    return 2


sys.exit(main())
