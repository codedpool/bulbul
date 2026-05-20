import React from "react";
import ReactDOM from "react-dom/client";
import { getCurrentWindow } from "@tauri-apps/api/window";

const root = ReactDOM.createRoot(document.getElementById("root"));

function renderError(label, err) {
  root.render(
    <div style={{ padding: 24, color: "#e6e6e6", background: "#0e0e0e", height: "100vh", fontFamily: "system-ui", fontSize: 13 }}>
      <h3 style={{ marginTop: 0 }}>Bulbul failed to load ({label})</h3>
      <pre style={{ color: "#d46b6b", whiteSpace: "pre-wrap" }}>{err?.stack || String(err)}</pre>
    </div>
  );
}

(async () => {
  let label = "main";
  try {
    label = getCurrentWindow().label;
  } catch {}

  try {
    if (label === "overlay") {
      const { default: Overlay } = await import("./Overlay.jsx");
      root.render(<React.StrictMode><Overlay /></React.StrictMode>);
    } else if (label === "scratchpad") {
      const { default: ScratchpadWindow } = await import("./ScratchpadWindow.jsx");
      root.render(<React.StrictMode><ScratchpadWindow /></React.StrictMode>);
    } else {
      const { default: App } = await import("./App.jsx");
      root.render(<React.StrictMode><App /></React.StrictMode>);
    }
  } catch (e) {
    renderError(label, e);
  }
})();
