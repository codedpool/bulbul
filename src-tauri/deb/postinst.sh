#!/bin/sh
# Grant Bulbul access to /dev/uinput so it can inject keystrokes on
# Wayland (where the compositor blocks every user-space typing path).
# Done narrowly: a dedicated system group owns the uinput node via a
# udev rule, and the Bulbul binary is setgid to that group — so Bulbul
# runs able to open uinput and nothing else. No relogin, no broad
# capability, no user action. The `sudo` the user already gave apt is
# the consent.
set -e

GROUP=bulbul-input
RULE=/etc/udev/rules.d/70-bulbul-uinput.rules

# 1. Dedicated system group that will own /dev/uinput.
if ! getent group "$GROUP" >/dev/null 2>&1; then
    groupadd --system "$GROUP" || true
fi

# 2. udev rule granting the uinput node to that group. static_node makes
#    it apply even before the module is first opened.
cat > "$RULE" <<EOF
KERNEL=="uinput", GROUP="$GROUP", MODE="0660", OPTIONS+="static_node=uinput"
EOF

# 3. setgid the installed binary so the process's group can open uinput
#    without the *user* being in the group (which would need a relogin).
#    Tauri may install the binary as either name depending on version.
for bin in /usr/bin/bulbul /usr/bin/Bulbul; do
    if [ -x "$bin" ]; then
        chgrp "$GROUP" "$bin" 2>/dev/null || true
        chmod 2755 "$bin" 2>/dev/null || true
    fi
done

# 4. Make it effective this boot without a reboot: load the module,
#    reload rules, and fix the live node's ownership.
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null \
    || udevadm trigger 2>/dev/null || true
if [ -e /dev/uinput ]; then
    chgrp "$GROUP" /dev/uinput 2>/dev/null || true
    chmod 0660 /dev/uinput 2>/dev/null || true
fi

exit 0
