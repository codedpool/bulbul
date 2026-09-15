// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

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
 * `image`         — optional imported banner photo. When present, the
 *                    card renders it as a full-bleed background instead
 *                    of the plain gradient, with title/blurb/samples
 *                    switching to a light-on-photo color set (forced,
 *                    not theme-dependent — the banner is a warm photo
 *                    regardless of light/dark mode) plus a scrim so text
 *                    stays legible over whatever part of the photo it
 *                    lands on.
 */
export default function FeatureHero({ title, samples, blurb, dismissKey, onSampleClick, image }) {
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
    <div
      className={`feature-hero${image ? " feature-hero-photo" : ""}`}
      // The scrim gradients that guarantee text legibility live in CSS
      // (.feature-hero-photo's background-image), stacked on top of this
      // photo — passed through as a custom property rather than a plain
      // inline backgroundImage so the inline style doesn't just replace
      // the stylesheet's own background-image and silently drop the scrim.
      style={image ? { "--feature-hero-image": `url(${image})` } : undefined}
    >
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
