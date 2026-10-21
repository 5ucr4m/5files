import React from "react";
import ReactDOM from "react-dom/client";
import App from "./App";
import { initBackend, isTauri } from "./lib/api";
import { setPlatform } from "./lib/platform";
import { restoreSession } from "./store/tabs";
import "./styles/index.css";

function forwardErrors() {
  if (!isTauri) return;
  const send = (level: string, message: string) => void import("@tauri-apps/api/core").then(({ invoke }) => invoke("frontend_log", { level, message }).catch(() => {}));
  window.addEventListener("error", (e) => send("error", `${e.message} @ ${e.filename}:${e.lineno}`));
  window.addEventListener("unhandledrejection", (e) => send("error", `unhandled rejection: ${String((e.reason && (e.reason.message ?? JSON.stringify(e.reason))) ?? e.reason)}`));
}

async function boot() {
  forwardErrors();
  const backend = await initBackend();
  setPlatform(await backend.platform());
  restoreSession();
  ReactDOM.createRoot(document.getElementById("root") as HTMLElement).render(
    <React.StrictMode>
      <App />
    </React.StrictMode>,
  );
  // The window starts hidden to avoid a white flash. A hidden webview doesn't run
  // requestAnimationFrame, so use a short timeout to let React commit before showing.
  setTimeout(() => void backend.window.show(), 30);
}

// Native context menu / text selection outside inputs feel out of place in a desktop app.
window.addEventListener("contextmenu", (e) => {
  if (!(e.target as HTMLElement).closest("input, textarea")) e.preventDefault();
});

void boot();
