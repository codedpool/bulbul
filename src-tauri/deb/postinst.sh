#!/bin/sh
# Grant Bulbul the input-device access it needs on Wayland, where the
# compositor blocks every user-space path. Bulbul talks to the kernel
# directly on both ends:
#   - reads /dev/input/event* (evdev) for an instant hold-to-talk hotkey
#   - writes /dev/uinput to type the transcribed text into any app
# This matters on Wayland (the compositor blocks every user-space path) and
# on X11 too — the in-process XTEST/clipboard path doesn't reliably deliver,
# so uinput is what actually makes typing work in both sessions.
#
# We deliberately do NOT setgid the binary (that runs it under glibc
# secure-execution mode, which breaks the WebKitGTK GUI).
set -e

RULE=/etc/udev/rules.d/70-bulbul-uinput.rules

# 1. udev rule. TAG+="uaccess" is the important part: systemd-logind grants
#    the user of the ACTIVE session an ACL on the device, which takes effect
#    IMMEDIATELY — no group membership involved, so no logout/login. That is
#    what removes the "log out and back in once to finish setup" step that
#    made Bulbul's first run on Linux feel broken next to Windows/macOS.
#    GROUP/MODE stay behind it as the fallback for systems without logind,
#    where the `input` group (step 2) is still the only mechanism.
#    static_node makes the rule apply before the module is first opened.
cat > "$RULE" <<'EOF'
KERNEL=="uinput", TAG+="uaccess", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"
EOF

# 2. Fallback path for systems without logind: add the human who ran the
#    install to `input`. $SUDO_USER is set because the installer runs
#    `sudo apt install`. This one only takes effect on their next login —
#    which is exactly why step 1's uaccess exists. (No reliable "installing
#    user" exists under bare apt automation, so we no-op when it's
#    unset/root — those users add themselves: `sudo usermod -aG input $USER`.)
if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ]; then
    usermod -aG input "$SUDO_USER" || true
fi

# 3. Make it effective RIGHT NOW: load the module, then reload+trigger the
#    rules so logind applies the uaccess ACL to the live node.
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null \
    || udevadm trigger 2>/dev/null || true
if [ -e /dev/uinput ]; then
    chgrp input /dev/uinput 2>/dev/null || true
    chmod 0660 /dev/uinput 2>/dev/null || true
    # Belt and braces: grant the installing user an ACL on the live node
    # directly, so access works in THIS session even if logind's uaccess
    # didn't fire (trigger raced, session not yet marked active, no acl
    # tooling, etc). The udev rule above is what makes it persist across
    # reboots; this is just about not making the user log out today.
    if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ]; then
        setfacl -m "u:$SUDO_USER:rw" /dev/uinput 2>/dev/null || true
    fi
fi

exit 0
