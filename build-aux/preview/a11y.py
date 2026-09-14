#!/usr/bin/env python3
"""Accessibility checks on the running preview, over AT-SPI.

  a11y.py names   every showing interactive widget has a name
  a11y.py focus   print the name of what has keyboard focus
"""
import sys

import pyatspi

INTERACTIVE = {
    pyatspi.ROLE_PUSH_BUTTON, pyatspi.ROLE_TOGGLE_BUTTON, pyatspi.ROLE_CHECK_BOX,
    pyatspi.ROLE_RADIO_BUTTON, pyatspi.ROLE_SLIDER, pyatspi.ROLE_COMBO_BOX,
    pyatspi.ROLE_SPIN_BUTTON, pyatspi.ROLE_MENU_ITEM,
}
SWITCH = getattr(pyatspi, "ROLE_SWITCH", None)
if SWITCH is not None:
    INTERACTIVE.add(SWITCH)


def app():
    desktop = pyatspi.Registry.getDesktop(0)
    for a in desktop:
        if a is not None and "obscura" in (a.name or "").lower():
            return a
    sys.exit("obscura is not on the accessibility bus")


def walk(node, path=()):
    for child in node:
        if child is None:
            continue
        yield child, path + (child.getRoleName(),)
        yield from walk(child, path + (child.getRoleName(),))


def label(node):
    if node.name:
        return node.name
    for rel in node.getRelationSet():
        if rel.getRelationType() == pyatspi.RELATION_LABELLED_BY and rel.getNTargets():
            return rel.getTarget(0).name
    return ""


if sys.argv[1] == "names":
    bad = 0
    for node, path in walk(app()):
        state = node.getState()
        if node.getRole() in INTERACTIVE and state.contains(pyatspi.STATE_SHOWING) and not label(node):
            print(f"unnamed {node.getRoleName()}: {' > '.join(path[-4:])}")
            bad += 1
    print(f"{bad} unnamed")
    sys.exit(min(bad, 100))
elif sys.argv[1] == "focus":
    for node, _ in walk(app()):
        if node.getState().contains(pyatspi.STATE_FOCUSED):
            print(label(node) or node.getRoleName())
            break
    else:
        print("-")
