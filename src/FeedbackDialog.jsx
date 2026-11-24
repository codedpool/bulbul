// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { IS_ANDROID } from "./platform.js";

const MAX_MESSAGE = 2000;
const MAX_NAME = 80;

// "Write to me": a short message to the developer. Sent through the Rust
// `send_feedback` command, which posts it to bulbultypes.xyz. Nothing but what
// is typed here (plus the app version and system name) leaves the device, and
// the email address only if the box is ticked.
export default function FeedbackDialog({ defaultName = "", onClose }) {
  const [name, setName] = useState(defaultName);
  const [message, setMessage] = useState("");
  const [shareEmail, setShareEmail] = useState(true);
  const [email, setEmail] = useState("");
  const [state, setState] = useState("idle"); // idle | sending | sent
  const [error, setError] = useState("");

  useEffect(() => {
    // Focus the message when a name is already filled in, otherwise the name.
    document.getElementById(defaultName ? "fb-message" : "fb-name")?.focus();
  }, [defaultName]);

  // When the WebView does report a smaller visible area (keyboard up), keep the
  // card inside it. Where it doesn't, the CSS cap and top position cover it.
  useEffect(() => {
    const vv = window.visualViewport;
    if (!vv) return undefined;
    const root = document.documentElement;
    const apply = () => root.style.setProperty("--fb-vh", `${vv.height}px`);
    apply();
    vv.addEventListener("resize", apply);
    return () => {
      vv.removeEventListener("resize", apply);
      root.style.removeProperty("--fb-vh");
    };
  }, []);

  useEffect(() => {
    const onKey = (e) => {
      if (e.key === "Escape" && state !== "sending") onClose();
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [onClose, state]);

  async function submit(e) {
    e.preventDefault();
    if (state === "sending") return;
    setError("");
    setState("sending");
    try {
      await invoke("send_feedback", {
        name,
        message,
        email: shareEmail ? email : null,
      });
      setState("sent");
    } catch (err) {
      setError(typeof err === "string" ? err : "Couldn't send your message. Please try again.");
      setState("idle");
    }
  }

  const canSend = name.trim() && message.trim() && state !== "sending" && (!shareEmail || email.trim());

  return (
    <div
      className="fb-backdrop"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget && state !== "sending") onClose();
      }}
    >
      <div className="fb-dialog" role="dialog" aria-modal="true" aria-labelledby="fb-title">
        {state === "sent" ? (
          <div className="fb-done">
            <h2 id="fb-title" className="fb-title">Message sent</h2>
            <p className="fb-text">
              Thank you.{" "}
              {shareEmail ? "I'll reply by email if your message needs an answer." : "I read every message, though without your email I can't reply."}
            </p>
            <div className="fb-actions">
              <button type="button" className="fb-primary" onClick={onClose} autoFocus>
                Close
              </button>
            </div>
          </div>
        ) : (
          <form onSubmit={submit} noValidate>
            <h2 id="fb-title" className="fb-title">Write to me</h2>
            <p className="fb-text fb-intro">
              Ideas, problems, or just a hello. It goes straight to the person who makes Bulbul.
            </p>

            <label htmlFor="fb-name" className="fb-label">Your name</label>
            <input
              id="fb-name"
              className="fb-input"
              value={name}
              maxLength={MAX_NAME}
              autoComplete="name"
              onChange={(e) => setName(e.target.value)}
            />

            <label htmlFor="fb-message" className="fb-label">Message</label>
            <textarea
              id="fb-message"
              className="fb-input fb-textarea"
              value={message}
              maxLength={MAX_MESSAGE}
              rows={IS_ANDROID ? 3 : 5}
              onChange={(e) => setMessage(e.target.value)}
            />
            <div className="fb-count" aria-live="off">{message.length} / {MAX_MESSAGE}</div>

            <label className="fb-check" htmlFor="fb-share">
              <input
                id="fb-share"
                type="checkbox"
                checked={shareEmail}
                onChange={(e) => setShareEmail(e.target.checked)}
              />
              <span>Share your email so I can reply</span>
            </label>
            {shareEmail ? (
              <>
                <label htmlFor="fb-email" className="fb-label">Your email</label>
                <input
                  id="fb-email"
                  type="email"
                  inputMode="email"
                  className="fb-input"
                  value={email}
                  autoComplete="email"
                  onChange={(e) => setEmail(e.target.value)}
                />
                <p className="fb-hint">I'll only use it to reply to you.</p>
              </>
            ) : (
              <p className="fb-hint">Without your email, I can't reply.</p>
            )}

            {error && (
              <p className="fb-error" role="alert">{error}</p>
            )}

            <p className="fb-hint">
              Only what you type here is sent, plus the app version and your operating system.
            </p>

            <div className="fb-actions">
              <button type="button" className="fb-secondary" onClick={onClose} disabled={state === "sending"}>
                Cancel
              </button>
              <button type="submit" className="fb-primary" disabled={!canSend}>
                {state === "sending" ? "Sending…" : "Send message"}
              </button>
            </div>
          </form>
        )}
      </div>
    </div>
  );
}
