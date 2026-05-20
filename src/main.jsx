import React from "react";
import ReactDOM from "react-dom/client";

const root = ReactDOM.createRoot(document.getElementById("root"));
const isOverlay = window.location.hash === "#overlay";

(async () => {
  if (isOverlay) {
    const { default: Overlay } = await import("./Overlay.jsx");
    root.render(
      <React.StrictMode>
        <Overlay />
      </React.StrictMode>,
    );
  } else {
    const { default: App } = await import("./App.jsx");
    root.render(
      <React.StrictMode>
        <App />
      </React.StrictMode>,
    );
  }
})();
