#!/bin/sh
# RPM counterpart of deb/postrm.sh. Remove the uinput udev rule Bulbul
# installed.
#
# RPM %postun runs on BOTH upgrade and final erase, with $1 = the number
# of package instances that will remain: 0 on final removal, 1 during an
# upgrade. On an upgrade the new package's %post has already reinstalled
# the rule, so we must NOT delete it here — only clean up when nothing
# remains.
set -e

# $1 is unset if a caller runs this directly; treat that as a real removal.
if [ "${1:-0}" != "0" ]; then
    exit 0
fi

RULE=/etc/udev/rules.d/70-bulbul-uinput.rules
if [ -f "$RULE" ]; then
    rm -f "$RULE"
    udevadm control --reload-rules 2>/dev/null || true
fi

exit 0
