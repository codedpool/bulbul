import { useEffect, useRef } from "react";
import "./ConfirmDialog.css";

/**
 * Themed confirmation/alert modal — replacement for the unstyled
 * `window.confirm()` / `window.alert()` browser dialogs so the in-app
 * dialogs match the rest of the UI.
 *
 * Controlled component: parent owns the open/closed state. Renders
 * nothing when `open` is false so it can be left mounted at the bottom
 * of a view without affecting layout.
 *
 * Self-contained styles (ConfirmDialog.css), so it works in both the
 * main app window and the standalone scratchpad pop-out — the pop-out
 * loads its own CSS bundle and doesn't pull App.css.
 *
 * Modes:
 *   - **Confirm** (default): two buttons, Cancel + Confirm. Cancel is
 *     focused by default so accidental Enter doesn't fire destructive
 *     actions. Escape / backdrop click both call `onCancel`.
 *   - **Alert**: pass `cancelLabel={null}` (or omit `onCancel`). Only
 *     the confirm button renders (label defaults to "OK"). Escape /
 *     backdrop click call `onConfirm` since there's no separate
 *     dismiss concept.
 */
export default function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel,
  cancelLabel = "Cancel",
  danger = false,
  onConfirm,
  onCancel,
}) {
  const focusRef = useRef(null);
  const isAlert = cancelLabel === null || onCancel == null;
  // Default the primary button label to "OK" in alert mode (single
  // dismissal button), "Confirm" in confirm mode (paired with Cancel).
  const resolvedConfirmLabel =
    confirmLabel ?? (isAlert ? "OK" : "Confirm");
  const dismiss = isAlert ? onConfirm : onCancel;

  useEffect(() => {
    if (!open) return;
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        dismiss?.();
      }
    };
    document.addEventListener("keydown", onKey);
    focusRef.current?.focus();
    return () => document.removeEventListener("keydown", onKey);
  }, [open, dismiss]);

  if (!open) return null;

  return (
    <div
      className="confirm-overlay"
      onMouseDown={(e) => {
        // Only treat as backdrop dismissal if the press started on the
        // backdrop itself, not on the modal card.
        if (e.target === e.currentTarget) dismiss?.();
      }}
    >
      <div
        className="confirm-card"
        role="dialog"
        aria-modal="true"
        aria-labelledby="confirm-modal-title"
      >
        <h2 id="confirm-modal-title">{title}</h2>
        {message && <p>{message}</p>}
        <div className="confirm-actions">
          {!isAlert && (
            <button ref={focusRef} onClick={onCancel}>{cancelLabel}</button>
          )}
          <button
            ref={isAlert ? focusRef : null}
            className={danger ? "danger" : "primary"}
            onClick={onConfirm}
          >
            {resolvedConfirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
