import { useEffect, useRef } from "react";
import "./ConfirmDialog.css";

/**
 * Themed confirmation modal — replacement for the unstyled `window.confirm()`
 * browser dialog so destructive actions match the rest of the app.
 *
 * Controlled component: parent owns the open/closed state. Renders nothing
 * when `open` is false so it can be left mounted at the bottom of a view
 * without affecting layout.
 *
 * Self-contained styles (ConfirmDialog.css), so it works in both the main
 * app window and the standalone scratchpad pop-out — which loads its own
 * CSS bundle and doesn't pull App.css.
 *
 * Interaction:
 *   - Escape cancels
 *   - Click on the backdrop cancels
 *   - Default focus on Cancel so a careless Enter doesn't fire a
 *     destructive action
 */
export default function ConfirmDialog({
  open,
  title,
  message,
  confirmLabel = "Confirm",
  cancelLabel = "Cancel",
  danger = false,
  onConfirm,
  onCancel,
}) {
  const cancelRef = useRef(null);

  useEffect(() => {
    if (!open) return;
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onCancel?.();
      }
    };
    document.addEventListener("keydown", onKey);
    cancelRef.current?.focus();
    return () => document.removeEventListener("keydown", onKey);
  }, [open, onCancel]);

  if (!open) return null;

  return (
    <div
      className="confirm-overlay"
      onMouseDown={(e) => {
        // Only treat as backdrop dismissal if the press started on the
        // backdrop itself, not on the modal card.
        if (e.target === e.currentTarget) onCancel?.();
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
          <button ref={cancelRef} onClick={onCancel}>{cancelLabel}</button>
          <button
            className={danger ? "danger" : "primary"}
            onClick={onConfirm}
          >
            {confirmLabel}
          </button>
        </div>
      </div>
    </div>
  );
}
