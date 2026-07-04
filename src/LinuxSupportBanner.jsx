import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Linux-only dashboard banner. The backend reports how the global
// hotkey ended up wired (Wayland portal / X11 poller / nowhere) plus
// which injection tools are installed; this surfaces the ones that
// need user action, with the exact command to run. Renders nothing
// when the session is fully working — most X11 and KDE users never
// see it.
//
// Dismissal is remembered per issue-set: fixing one problem (or a
// backend regression creating a new one) resurfaces the banner.
const DISMISS_KEY = "bulbul-linux-banner-dismissed";

export default function LinuxSupportBanner() {
  const [info, setInfo] = useState(null);
  const [status, setStatus] = useState(null); // {backend, detail}
  const [dismissed, setDismissed] = useState(
    () => localStorage.getItem(DISMISS_KEY) || "",
  );

  useEffect(() => {
    let mounted = true;
    invoke("get_linux_support_info")
      .then((v) => {
        if (!mounted || !v) return;
        setInfo(v);
        if (v.hotkey_backend && v.hotkey_backend !== "unknown") {
          setStatus({ backend: v.hotkey_backend, detail: v.hotkey_detail });
        }
      })
      .catch(() => {});
    const un = listen("linux-hotkey-status", (e) => {
      if (mounted && e.payload) setStatus(e.payload);
    });
    return () => {
      mounted = false;
      un.then((f) => f()).catch(() => {});
    };
  }, []);

  if (!info) return null;

  const issues = [];

  if (status?.backend === "none") {
    issues.push({
      key: "hotkey",
      text: status.detail || "The dictation hotkey couldn't be registered.",
      command: info.toggle_command,
      commandHint:
        "Bind this command to a keyboard shortcut in your system settings — press once to start dictating, again to stop.",
    });
  }

  if (info.wayland && !info.wtype && !info.ydotool) {
    issues.push({
      key: "paste",
      text: info.gnome
        ? "Pasting into Wayland apps needs ydotool on GNOME (its compositor blocks the tool Bulbul prefers)."
        : "Pasting into Wayland apps needs a keystroke tool.",
      command: info.gnome
        ? "sudo apt install ydotool && systemctl --user enable --now ydotool.service"
        : "sudo apt install wtype",
    });
  }

  if (info.gnome) {
    issues.push({
      key: "tray",
      text: "GNOME hides tray icons by default — install the “AppIndicator and KStatusNotifierItem” Shell extension to see Bulbul in the top bar. Dictation works either way.",
    });
  }

  if (issues.length === 0) return null;

  const fingerprint = issues.map((i) => i.key).join(",");
  if (dismissed === fingerprint) return null;

  return (
    <div className="linux-banner" role="status">
      <div className="linux-banner-head">
        <span className="linux-banner-dot" aria-hidden />
        <strong>Linux setup{info.wayland ? " (Wayland session)" : ""}</strong>
        <button
          className="linux-banner-dismiss"
          onClick={() => {
            localStorage.setItem(DISMISS_KEY, fingerprint);
            setDismissed(fingerprint);
          }}
          aria-label="Dismiss"
          title="Dismiss until something changes"
        >
          ✕
        </button>
      </div>
      <ul className="linux-banner-list">
        {issues.map((i) => (
          <li key={i.key}>
            {i.text}
            {i.command && (
              <>
                {i.commandHint ? ` ${i.commandHint}` : null}
                <code className="linux-banner-code">{i.command}</code>
              </>
            )}
          </li>
        ))}
      </ul>
    </div>
  );
}
