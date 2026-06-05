import { useState } from "react";

/**
 * Reusable compact hero strip for feature pages. Lives between the page
 * header and the page content. Each instance dismisses to localStorage
 * under its own `dismissKey` so the user only dismisses once per page.
 *
 * `title`         — string or ReactNode (use <em> to highlight a word).
 * `samples`       — optional [{ trigger, expansion }] rows shown as
 *                    `"trigger"` → `expansion` pill pairs.
 * `blurb`         — optional one-liner shown below the title when there
 *                    are no samples.
 * `dismissKey`    — unique localStorage key.
 * `onSampleClick` — optional. When provided, each sample row becomes a
 *                    button that calls `onSampleClick(sample)`. Lets a
 *                    page (e.g. Snippets) wire the examples into "open
 *                    the add-new form pre-filled with this row".
 */
export default function FeatureHero({ title, samples, blurb, dismissKey, onSampleClick }) {
  const [visible, setVisible] = useState(() => {
    try { return localStorage.getItem(dismissKey) !== "1"; }
    catch { return true; }
  });

  if (!visible) return null;

  function dismiss() {
    setVisible(false);
    try { localStorage.setItem(dismissKey, "1"); } catch {}
  }

  return (
    <div className="feature-hero">
      <button
        className="feature-hero-close"
        onClick={dismiss}
        aria-label="Dismiss"
        title="Dismiss"
      >
        <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
          <line x1="6" y1="6" x2="18" y2="18" />
          <line x1="18" y1="6" x2="6" y2="18" />
        </svg>
      </button>
      <h2 className="feature-hero-title">{title}</h2>
      {samples && samples.length > 0 && (
        <div className="feature-hero-samples">
          {samples.map((s, i) => {
            const inner = (
              <>
                <span className="feature-hero-trigger">"{s.trigger}"</span>
                <span className="feature-hero-arrow">→</span>
                <span className="feature-hero-expansion">{s.expansion}</span>
              </>
            );
            return onSampleClick ? (
              <button
                type="button"
                className="feature-hero-row feature-hero-row-clickable"
                key={i}
                onClick={() => onSampleClick(s)}
              >
                {inner}
              </button>
            ) : (
              <div className="feature-hero-row" key={i}>
                {inner}
              </div>
            );
          })}
        </div>
      )}
      {blurb && !samples && (
        <p className="feature-hero-blurb">{blurb}</p>
      )}
    </div>
  );
}
