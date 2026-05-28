import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Theme preference is one of: "dark" | "light" | "system".
// "system" follows the OS via prefers-color-scheme and tracks live changes.
// The resolved concrete theme ("dark" | "light") is written to
// <html data-theme="…">, which theme.css keys off of.

const STORAGE_KEY = "bulbul-theme";
const mq = window.matchMedia("(prefers-color-scheme: dark)");
let currentPref = "dark";
let listenersWired = false;

function resolve(pref) {
  if (pref === "system") return mq.matches ? "dark" : "light";
  return pref === "light" ? "light" : "dark";
}

function setAttr(pref) {
  currentPref = pref === "light" || pref === "system" ? pref : "dark";
  document.documentElement.setAttribute("data-theme", resolve(currentPref));
  try {
    localStorage.setItem(STORAGE_KEY, currentPref);
  } catch {}
}

function onSystemChange() {
  if (currentPref === "system") setAttr("system");
}

/** Current stored preference ("dark" | "light" | "system"). */
export function currentTheme() {
  return currentPref;
}

/** Concrete theme actually showing ("dark" | "light"). */
export function resolvedTheme() {
  return resolve(currentPref);
}

/** Apply a preference immediately (instant feedback, no round-trip). */
export function applyTheme(pref) {
  setAttr(pref);
}

/**
 * Wire up theming for the current window. Runs a synchronous best-guess
 * from the localStorage cache first (so there's no flash of the wrong
 * theme), then reconciles with the persisted config, and listens for
 * cross-window changes + OS theme changes.
 */
export function initTheme() {
  // Synchronous prime from cache — happens before any await.
  let cached = "dark";
  try {
    cached = localStorage.getItem(STORAGE_KEY) || "dark";
  } catch {}
  setAttr(cached);

  if (!listenersWired) {
    listenersWired = true;
    if (mq.addEventListener) mq.addEventListener("change", onSystemChange);
    else if (mq.addListener) mq.addListener(onSystemChange);
    listen("theme-changed", (e) => setAttr(e.payload || "dark")).catch(() => {});
  }

  // Reconcile with the source of truth (config) in the background.
  invoke("get_config")
    .then((cfg) => setAttr(cfg?.theme || "dark"))
    .catch(() => {});
}
