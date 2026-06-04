import { useEffect, useRef, useState } from "react";
import "./Combobox.css";

/**
 * Themed single-select dropdown. Reusable replacement for the native
 * `<select>` element, whose popup list always renders with the OS-native
 * widget regardless of styling on the closed control. This component
 * builds the whole thing in DOM so it matches the rest of the app — and
 * actually scrolls when the option list is long.
 *
 * Self-contained styles (Combobox.css) so it works in any window bundle.
 *
 * Props:
 *   value      — the selected option's `code`
 *   options    — Array<{ code: string, label: string }>
 *   onChange   — (code: string) => void
 *   disabled   — optional, disables interaction and dims the control
 *   width      — optional, CSS width (string or number); default 240px
 *   ariaLabel  — optional, accessible name when there's no visible label
 */
// Matches max-height of .combobox-list in Combobox.css. If you change
// one, change the other — the JS uses this to decide whether to flip
// the popup above the trigger when there isn't enough room below.
const LIST_MAX_HEIGHT = 240;

export default function Combobox({
  value,
  options,
  onChange,
  disabled = false,
  width = 240,
  ariaLabel,
}) {
  const [open, setOpen] = useState(false);
  // "down" puts the popup below the trigger (default); "up" anchors it
  // above. Decided at open time based on viewport space.
  const [direction, setDirection] = useState("down");
  const rootRef = useRef(null);
  const triggerRef = useRef(null);

  useEffect(() => {
    if (!open) return;
    const onDocDown = (e) => {
      if (rootRef.current && !rootRef.current.contains(e.target)) setOpen(false);
    };
    const onKey = (e) => {
      if (e.key === "Escape") setOpen(false);
    };
    document.addEventListener("mousedown", onDocDown);
    document.addEventListener("keydown", onKey);
    return () => {
      document.removeEventListener("mousedown", onDocDown);
      document.removeEventListener("keydown", onKey);
    };
  }, [open]);

  // Close on disable transition (e.g. parent toggles off while open).
  useEffect(() => {
    if (disabled && open) setOpen(false);
  }, [disabled, open]);

  function decideDirection() {
    const rect = triggerRef.current?.getBoundingClientRect();
    if (!rect) return "down";
    const spaceBelow = window.innerHeight - rect.bottom;
    const spaceAbove = rect.top;
    // Flip up only if below truly can't fit AND above has more room.
    // Keeps the default "open downward" feel for the common case.
    if (spaceBelow < LIST_MAX_HEIGHT && spaceAbove > spaceBelow) return "up";
    return "down";
  }

  function toggle(e) {
    e.preventDefault();
    e.stopPropagation();
    if (disabled) return;
    setOpen((wasOpen) => {
      if (!wasOpen) setDirection(decideDirection());
      return !wasOpen;
    });
  }

  const selected = options.find((o) => o.code === value) || options[0];
  const widthStyle = typeof width === "number" ? `${width}px` : width;

  return (
    <div
      className={`combobox ${disabled ? "disabled" : ""}`}
      ref={rootRef}
      style={{ width: widthStyle }}
    >
      <button
        ref={triggerRef}
        type="button"
        className={`combobox-trigger ${open ? "open" : ""}`}
        disabled={disabled}
        onClick={toggle}
        aria-haspopup="listbox"
        aria-expanded={open}
        aria-label={ariaLabel}
      >
        <span className="combobox-value">{selected?.label ?? ""}</span>
        <ComboChevron />
      </button>
      {open && (
        <div className={`combobox-list ${direction === "up" ? "up" : ""}`} role="listbox">
          {options.map((o) => (
            <button
              key={o.code}
              type="button"
              role="option"
              aria-selected={o.code === value}
              className={`combobox-item ${o.code === value ? "selected" : ""}`}
              onClick={(e) => {
                e.preventDefault();
                e.stopPropagation();
                onChange(o.code);
                setOpen(false);
              }}
            >
              {o.label}
            </button>
          ))}
        </div>
      )}
    </div>
  );
}

function ComboChevron() {
  return (
    <svg width="10" height="6" viewBox="0 0 10 6" aria-hidden>
      <path
        d="M1 1l4 4 4-4"
        fill="none"
        stroke="currentColor"
        strokeWidth="1.4"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  );
}
