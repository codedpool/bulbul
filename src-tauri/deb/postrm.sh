#!/bin/sh
# Remove the uinput udev rule Bulbul installed. Leave the bulbul-input
# system group in place — deleting it risks dangling GIDs on files and
# it's harmless empty.
set -e

RULE=/etc/udev/rules.d/70-bulbul-uinput.rules
if [ -f "$RULE" ]; then
    rm -f "$RULE"
    udevadm control --reload-rules 2>/dev/null || true
fi

exit 0
