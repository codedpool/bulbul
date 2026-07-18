#!/bin/sh
# RPM counterpart of deb/postinst.sh. Grants Bulbul the input-device
# access it needs on Wayland (and on X11, where the in-process
# XTEST/clipboard path doesn't reliably deliver), so hold-to-talk and
# typing into other apps work on Fedora/openSUSE the same way they do on
# Debian/Ubuntu — no logout/login.
#
# Why a separate script from the .deb's: RPM %post scriptlets run as root
# with NO $SUDO_USER (dnf/zypper don't set it), so the deb's
# "$SUDO_USER"-driven immediate ACL bridge would silently no-op. We find
# the active graphical session's user via loginctl instead. The udev
# `uaccess` rule below is the primary mechanism and doesn't need to know
# the user at all — logind grants the ACL to whoever owns the active
# session; the loginctl bridge is just belt-and-braces for the current
# session if the trigger races.
set -e

RULE=/etc/udev/rules.d/70-bulbul-uinput.rules

# 1. udev rule. TAG+="uaccess" makes systemd-logind grant the user of the
#    ACTIVE session an ACL on /dev/uinput immediately — no group, no
#    logout. GROUP/MODE stay behind it as the fallback for systems without
#    logind. static_node makes the rule apply before the module is first
#    opened.
cat > "$RULE" <<'EOF'
KERNEL=="uinput", TAG+="uaccess", GROUP="input", MODE="0660", OPTIONS+="static_node=uinput"
EOF
# Fedora/SELinux: a file written by a %post scriptlet can land with the
# wrong SELinux label; restore the correct one so udev reads it. No-op on
# non-SELinux systems.
if command -v restorecon >/dev/null 2>&1; then
    restorecon "$RULE" 2>/dev/null || true
fi

# 2. Find the active graphical session's user for the immediate-access
#    bridge (RPM gives us no $SUDO_USER). loginctl's per-property
#    `--value` output is stable across systemd versions, unlike the
#    column layout of `list-sessions`.
ACTIVE_USER=""
if command -v loginctl >/dev/null 2>&1; then
    for sid in $(loginctl list-sessions --no-legend 2>/dev/null | awk '{print $1}'); do
        active=$(loginctl show-session "$sid" -p Active --value 2>/dev/null || true)
        remote=$(loginctl show-session "$sid" -p Remote --value 2>/dev/null || true)
        uname=$(loginctl show-session "$sid" -p Name --value 2>/dev/null || true)
        if [ "$active" = "yes" ] && [ "$remote" = "no" ] \
            && [ -n "$uname" ] && [ "$uname" != "root" ]; then
            ACTIVE_USER="$uname"
            break
        fi
    done
fi

# 3. Fallback path for systems without logind uaccess: add the active user
#    to `input`. Takes effect on their next login — step 1's uaccess is
#    what avoids needing that.
if [ -n "$ACTIVE_USER" ]; then
    usermod -aG input "$ACTIVE_USER" 2>/dev/null || true
fi

# 4. Make it effective RIGHT NOW: load the module, reload+trigger the
#    rules so logind applies the uaccess ACL to the live node, and settle
#    so the ACL lands before Bulbul first probes /dev/uinput.
modprobe uinput 2>/dev/null || true
udevadm control --reload-rules 2>/dev/null || true
udevadm trigger --subsystem-match=misc --attr-match=name=uinput 2>/dev/null \
    || udevadm trigger 2>/dev/null || true
udevadm settle --timeout=5 2>/dev/null || true
if [ -e /dev/uinput ]; then
    chgrp input /dev/uinput 2>/dev/null || true
    chmod 0660 /dev/uinput 2>/dev/null || true
    # Belt-and-braces: grant the active user an ACL on the live node so
    # access works in THIS session even if logind's uaccess didn't fire.
    if [ -n "$ACTIVE_USER" ]; then
        setfacl -m "u:$ACTIVE_USER:rw" /dev/uinput 2>/dev/null || true
    fi
fi

# 5. Same immediate-access bridge for the KEYBOARD devices (evdev), so the
#    hold-to-talk hotkey works in THIS session without the input-group
#    relogin. /dev/input/event* is already group `input`, so this grants
#    exactly what the group would, applied to the current session now.
if [ -n "$ACTIVE_USER" ]; then
    for dev in /dev/input/event*; do
        [ -e "$dev" ] && setfacl -m "u:$ACTIVE_USER:rw" "$dev" 2>/dev/null || true
    done
fi

exit 0
