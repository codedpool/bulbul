import { useEffect, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import "./Overlay.css";

export default function Overlay() {
  const [status, setStatus] = useState({ state: "idle", message: null });

  useEffect(() => {
    document.body.style.background = "transparent";
    document.documentElement.style.background = "transparent";
    const un = listen("bulbul-status", (e) => setStatus(e.payload));
    return () => { un.then((f) => f()); };
  }, []);

  const expanded = status.state !== "idle";

  return (
    <div className={`overlay ${expanded ? "expanded" : "collapsed"}`}>
      <div className={`pill pill-${status.state}`}>
        <span className="pill-icon">{renderIcon(status.state)}</span>
        {expanded && <span className="pill-label">{label(status.state)}</span>}
      </div>
    </div>
  );
}

function renderIcon(state) {
  switch (state) {
    case "listening":
      return (
        <div className="bars" aria-hidden>
          <span /><span /><span /><span />
        </div>
      );
    case "processing":
    case "injecting":
      return <div className="spinner" aria-hidden />;
    case "done":
      return <span className="glyph">✓</span>;
    case "error":
      return <span className="glyph">!</span>;
    default:
      return <span className="dot" aria-hidden />;
  }
}

function label(state) {
  switch (state) {
    case "listening": return "Listening";
    case "processing": return "Transcribing";
    case "injecting": return "Inserting";
    case "done": return "Done";
    case "error": return "Error";
    default: return "";
  }
}
