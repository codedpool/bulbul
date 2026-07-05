#!/bin/sh
# Grant Bulbul the input-device access it needs on Wayland, where the
# compositor blocks every user-space path. Bulbul talks to the kernel
# directly on both ends:
#   - reads /dev/input/event* (evdev) for an instant hold-to-talk hotkey
#   - writes /dev/uinput to type the transcribed text into any app
# Both live behind the standard `input` group. /dev/input/event* is
# already group `input` on Ubuntu; we add a udev rule so /dev/uinput is
# too, then add the installing user to `input`. One logout/login and it
# all works — no per-app prompts, no custom shortcut.
#
# We deliberately do NOT setgid the binary (that runs it under glibc
# secure-execution mode, which breaks the WebKitGTK GUI).
set -e

RULE=/etc/udev/rules.d/70-bulbul-uinput.rules

# 1. udev rule: put /dev/uinput in the `input` group (mode 0660).
#    static_node makes it apply even before the module is first opened.
cat > "$RULE" <<'EOF'
KERNEL=="uinput", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"
EOF

# 2. Add the human who ran the install to `input`. $SUDO_USER is set
#    because the installer runs `sudo apt install`. Takes effect on their
#    next login. (No reliable "installing user" exists under bare apt
#    automation, so we no-op when it's unset/root — those users add
#    themselves: `sudo usermod -aG input $USER`.)
if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ]; then
    usermod -aG input "$SUDO_USER" || true
fi

# 3. Make the rule effective this boot: load the module, reload rules,
#    fix the live node. (Group membership from step 2 still needs the
#    user's next login — nothing here can shortcut that.)
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null \
    || udevadm trigger 2>/dev/null || true
if [ -e /dev/uinput ]; then
    chgrp input /dev/uinput 2>/dev/null || true
    chmod 0660 /dev/uinput 2>/dev/null || true
fi

exit 0
