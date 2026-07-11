import { useState } from "react";

/**
 * Accent-tinted "how to use this" card with a close button. Dismissal is
 * SESSION-only (sessionStorage, keyed by `storageKey`): it stays hidden as
 * you navigate around the app, but reappears the next time the app is
 * relaunched — unlike FeatureHero, which dismisses permanently.
 *
 * `title`      — heading text.
 * `storageKey` — unique sessionStorage key.
 * `children`   — the steps / tip body (e.g. an <ol> plus a tip <p>).
 */
export default function HowToCard({ title, storageKey, children }) {
  const [visible, setVisible] = useState(() => {
    try {
      return sessionStorage.getItem(storageKey) !== "1";
    } catch {
      return true;
    }
  });

  if (!visible) return null;

  function dismiss() {
    setVisible(false);
    try {
      sessionStorage.setItem(storageKey, "1");
    } catch {}
  }

  return (
    <div className="howto-card">
      <button
        className="howto-close"
        onClick={dismiss}
        aria-label="Dismiss"
        title="Dismiss (shows again next launch)"
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <line x1="6" y1="6" x2="18" y2="18" />
          <line x1="18" y1="6" x2="6" y2="18" />
        </svg>
      </button>
      <div className="howto-title">{title}</div>
      {children}
    </div>
  );
}
