#!/usr/bin/env python3
"""Keep a mutter remote-desktop session (it dies with the D-Bus client that
made it) and feed it the commands written to FIFO, one per line:
point X Y | down | up | key KEYSYM."""
import os
import sys

from gi.repository import Gio, GLib

fifo = sys.argv[1]
bus = Gio.bus_get_sync(Gio.BusType.SESSION)
SESSION = "org.gnome.Mutter.RemoteDesktop.Session"


def call(path, interface, method, args=None, reply=None):
    return bus.call_sync("org.gnome.Mutter.RemoteDesktop", path, interface, method, args,
                         reply, Gio.DBusCallFlags.NONE, 5000, None)


session = call("/org/gnome/Mutter/RemoteDesktop", "org.gnome.Mutter.RemoteDesktop",
               "CreateSession", None, GLib.VariantType("(o)")).unpack()[0]
call(session, SESSION, "Start")
# The first key after a session starts can be lost while mutter sets up the
# virtual keyboard: spend it on a Shift.
for pressed in (True, False):
    call(session, SESSION, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (0xffe1, pressed)))
if not os.path.exists(fifo):
    os.mkfifo(fifo)
print("ready", flush=True)
while True:
    with open(fifo) as commands:
        for line in commands:
            word = line.split()
            if not word:
                continue
            if word[0] == "point":
                # From the top-left corner, so the moves are absolute.
                call(session, SESSION, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (-10000.0, -10000.0)))
                call(session, SESSION, "NotifyPointerMotionRelative", GLib.Variant("(dd)", (float(word[1]), float(word[2]))))
            elif word[0] in ("down", "up"):
                call(session, SESSION, "NotifyPointerButton", GLib.Variant("(ib)", (272, word[0] == "down")))  # BTN_LEFT
            elif word[0] == "key":
                for pressed in (True, False):
                    call(session, SESSION, "NotifyKeyboardKeysym", GLib.Variant("(ub)", (int(word[1], 0), pressed)))
