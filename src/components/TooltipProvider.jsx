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

      // Skip rendering when the title text just repeats the element's
      // visible label — those add zero information and turn every named
      // button into a hover-bubble. The attribute is still stripped
      // above so the OS-native tooltip doesn't appear for these either.
      // Icon-only buttons (no visible text) always pass this gate.
      const visible = (target.textContent || "").trim();
      if (visible && normalize(visible) === normalize(text)) {
        activeTarget = target;
        return;
      }

      const rect = target.getBoundingClientRect();
      const spaceAbove = rect.top;
      const placement = spaceAbove > 50 ? "top" : "bottom";
      const x = rect.left + rect.width / 2;
      const y = placement === "top" ? rect.top : rect.bottom;

      setTip({ text, x, y, placement });
      activeTarget = target;
    }

    function normalize(s) {
      return s.trim().toLowerCase().replace(/\s+/g, " ");
    }

    function dismiss() {
      if (activeTarget) teardown(activeTarget);
      clearTimeout(showTimer);
      setTip(null);
      activeTarget = null;
    }

    function onOver(e) {
      // Defensive sweep: if a tooltip is active but the cursor is no
      // longer inside its target, dismiss it. Catches the cases where
      // mouseout was missed — fast cursor moves, the source element
      // being unmounted by a re-render, or the cursor crossing a
      // portal'd boundary. This is what was leaving a stale "Open at
      // startup" tooltip floating on the Home page after the user had
      // moved well away from the sidebar toggle.
      if (activeTarget && !activeTarget.contains(e.target)) {
        dismiss();
      }

      const target = e.target.closest("[title]");
      if (!target || target === activeTarget) return;
      clearTimeout(showTimer);
      showTimer = setTimeout(() => showFor(target), SHOW_DELAY_MS);
    }

    function onOut(e) {
      // mouseout bubbles, so it fires every time the cursor crosses a
      // child boundary inside the same title-bearing element. Without
      // this guard, the tooltip would dismiss + re-arm every time the
      // cursor passed over an inner SVG, span, etc., producing a
      // flicker / "lingering ghost" effect on rich buttons.
      if (
        activeTarget &&
        e.relatedTarget instanceof Node &&
        activeTarget.contains(e.relatedTarget)
      ) {
        return;
      }

      // Truly leaving the active target (or no active target yet).
      // Restore its title attribute and dismiss any showing / pending
      // tooltip.
      if (activeTarget) {
        teardown(activeTarget);
      } else {
        // Pre-show: timer pending but nothing stashed. teardown on the
        // leaving element is a no-op unless something odd happened.
        teardown(e.target);
      }
      clearTimeout(showTimer);
      setTip(null);
      activeTarget = null;
    }

    // Any click dismisses an open tooltip immediately. This covers two
    // cases mouseout can't: (1) the target unmounts as a result of the
    // click (e.g., navigating to a different view) so no mouseout ever
    // fires — without this the pill, which is portalled to <body>,
    // would survive and float over the new view; (2) the user has
    // clearly stopped reading the hint once they've committed to the
    // action. Capture phase so it lands before any handler that
    // unmounts the source element.
    function onDown() { dismiss(); }

    // Cursor left the window entirely (alt-tabbed away, dragged to the
    // taskbar, moved off the WebView). mouseout's relatedTarget can be
    // null in this case but isn't always reliable — mouseleave on
    // document is.
    function onLeave() { dismiss(); }

    document.addEventListener("mouseover", onOver);
    document.addEventListener("mouseout", onOut);
    document.addEventListener("mousedown", onDown, true);
    document.addEventListener("mouseleave", onLeave);
    window.addEventListener("blur", onLeave);
    return () => {
      document.removeEventListener("mouseover", onOver);
      document.removeEventListener("mouseout", onOut);
      document.removeEventListener("mousedown", onDown, true);
      document.removeEventListener("mouseleave", onLeave);
      window.removeEventListener("blur", onLeave);
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
