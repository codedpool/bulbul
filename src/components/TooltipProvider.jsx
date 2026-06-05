import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import "./TooltipProvider.css";

/**
 * Global themed-tooltip provider. Mount once at the root of a React
 * tree (App, ScratchpadWindow) and it transparently replaces the
 * browser's default `title=` tooltip with an in-theme popup.
 *
 * How it works:
 *   - One document-level mouseover listener finds the nearest element
 *     with a `title` attribute under the cursor.
 *   - After a short hover delay it strips the `title` (suppressing the
 *     OS-native tooltip) and renders a themed pill anchored to the
 *     element. The original `title` is stashed on the element via a
 *     WeakMap so it can be restored on mouseout — meaning accessibility
 *     tools (screen readers, keyboard focus) still see the label.
 *   - No JSX changes required — every existing `title=` everywhere in
 *     the app picks this up automatically.
 *
 * Placement prefers top; flips to bottom when the anchor is too close
 * to the top of the viewport.
 */
const SHOW_DELAY_MS = 400;
const stashed = new WeakMap();

export default function TooltipProvider() {
  const [tip, setTip] = useState(null);
  // tip = { text, x, y, placement } | null

  useEffect(() => {
    let showTimer = null;
    let activeTarget = null;

    function teardown(target) {
      if (target && stashed.has(target)) {
        target.setAttribute("title", stashed.get(target));
        stashed.delete(target);
      }
    }

    function showFor(target) {
      const text = target.getAttribute("title");
      if (!text || !text.trim()) return;
      stashed.set(target, text);
      // Removing the attribute prevents the native tooltip from ever
      // appearing. It will be restored on mouseout.
      target.removeAttribute("title");

      const rect = target.getBoundingClientRect();
      const spaceAbove = rect.top;
      const placement = spaceAbove > 50 ? "top" : "bottom";
      const x = rect.left + rect.width / 2;
      const y = placement === "top" ? rect.top : rect.bottom;

      setTip({ text, x, y, placement });
      activeTarget = target;
    }

    function onOver(e) {
      const target = e.target.closest("[title]");
      if (!target || target === activeTarget) return;
      clearTimeout(showTimer);
      showTimer = setTimeout(() => showFor(target), SHOW_DELAY_MS);
    }

    function onOut(e) {
      const left = e.target.closest("*");
      // Always restore the title on the element being left — covers the
      // pre-delay case where we haven't shown a tip yet but still want
      // the attribute back if it was stashed.
      teardown(left);
      if (left === activeTarget || !activeTarget) {
        clearTimeout(showTimer);
        setTip(null);
        activeTarget = null;
      }
    }

    document.addEventListener("mouseover", onOver);
    document.addEventListener("mouseout", onOut);
    return () => {
      document.removeEventListener("mouseover", onOver);
      document.removeEventListener("mouseout", onOut);
      clearTimeout(showTimer);
      teardown(activeTarget);
    };
  }, []);

  if (!tip) return null;
  return createPortal(<TooltipNode tip={tip} />, document.body);
}

/// Renders the pill and adjusts horizontal position post-layout so it
/// stays inside the viewport. Sidebar-anchored tooltips would otherwise
/// extend leftward past the window edge (the .placement-* CSS centres
/// the pill on the anchor, with no overflow guard).
function TooltipNode({ tip }) {
  const ref = useRef(null);
  const [shift, setShift] = useState(0); // px to nudge horizontally

  useLayoutEffect(() => {
    setShift(0); // reset between tooltips
    const el = ref.current;
    if (!el) return;
    const rect = el.getBoundingClientRect();
    const PAD = 8;
    let dx = 0;
    if (rect.left < PAD) dx = PAD - rect.left;
    else if (rect.right > window.innerWidth - PAD)
      dx = window.innerWidth - PAD - rect.right;
    if (dx !== 0) setShift(dx);
  }, [tip.text, tip.x, tip.y, tip.placement]);

  return (
    <div
      ref={ref}
      className={`app-tooltip placement-${tip.placement}`}
      style={{
        left: `${tip.x}px`,
        top: `${tip.y}px`,
        // Stacked with the centring translate from CSS via a CSS var so
        // the JS shift composes cleanly without overriding the
        // placement-specific Y offset.
        "--tooltip-shift-x": `${shift}px`,
      }}
      role="tooltip"
    >
      {tip.text}
    </div>
  );
}
