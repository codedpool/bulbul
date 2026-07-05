#!/bin/sh
# Grant Bulbul access to /dev/uinput so it can inject keystrokes on
# Wayland (where the compositor blocks every user-space typing path).
#
# We deliberately do NOT setgid the Bulbul binary: a setgid executable
# runs under glibc "secure-execution mode" (AT_SECURE), which breaks the
# WebKitGTK GUI — the app won't launch. Instead we add the installing
# user to a dedicated group that owns /dev/uinput. That needs one
# logout/login to take effect, then typing works everywhere with no
# further prompts.
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

# 3. Add the human who ran the install to the group. $SUDO_USER is set
#    because the installer runs `sudo apt install`. Takes effect on their
#    next login. (No reliable "installing user" exists under bare apt
#    automation, so we no-op when $SUDO_USER is unset/root — those users
#    can add themselves: `sudo usermod -aG bulbul-input $USER`.)
if [ -n "$SUDO_USER" ] && [ "$SUDO_USER" != "root" ]; then
    usermod -aG "$GROUP" "$SUDO_USER" || true
fi

# 4. Make the rule effective this boot: load the module, reload rules,
#    and fix the live node's ownership. (Group membership from step 3
#    still needs the user's next login — nothing here can shortcut that.)
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null \
    || udevadm trigger 2>/dev/null || true
if [ -e /dev/uinput ]; then
    chgrp "$GROUP" /dev/uinput 2>/dev/null || true
    chmod 0660 /dev/uinput 2>/dev/null || true
fi

exit 0
