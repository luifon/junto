# usage: drag_source.py <x> <y> <file>...; opens a small GTK 3 window at x,y that starts an XDND file drag when its area is dragged
import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("Gdk", "3.0")
from gi.repository import Gdk, GLib, Gtk

x, y, paths = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3:]

window = Gtk.Window(title="Junto spike drag source")
window.set_default_size(260, 120)
window.set_keep_above(True)
area = Gtk.EventBox()
area.add(Gtk.Label(label="drag from here: %d files" % len(paths)))
area.drag_source_set(Gdk.ModifierType.BUTTON1_MASK, [], Gdk.DragAction.COPY)
area.drag_source_add_uri_targets()


def on_get(widget, context, selection, info, time):
    uris = [GLib.filename_to_uri(path, None) for path in paths]
    accepted = selection.set_uris(uris)
    print("drag source: data requested, target", selection.get_target().name(), "set", accepted, uris, flush=True)


def on_end(widget, context):
    print("drag source: drag ended", flush=True)


area.connect("drag-begin", lambda *a: print("drag source: drag began", flush=True))
area.connect("drag-data-get", on_get)
area.connect("drag-end", on_end)
window.add(area)
window.connect("destroy", Gtk.main_quit)
window.show_all()
window.move(x, y)
GLib.timeout_add_seconds(int(120), Gtk.main_quit)
Gtk.main()
