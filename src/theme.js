import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

// Theme preference is one of: "dark" | "light" | "system".
// "system" follows the OS via prefers-color-scheme and tracks live changes.
// The resolved concrete theme ("dark" | "light") is written to
// <html data-theme="…">, which theme.css keys off of.

const STORAGE_KEY = "bulbul-theme";
const ANIM_CLASS = "theme-anim";
const ANIM_MS = 480;
const mq = window.matchMedia("(prefers-color-scheme: dark)");
let currentPref = "light";
let listenersWired = false;
let animTimer = null;

function resolve(pref) {
  if (pref === "system") return mq.matches ? "dark" : "light";
  return pref === "dark" ? "dark" : "light";
}

function setAttr(pref) {
  currentPref = pref === "dark" || pref === "system" ? pref : "light";
  document.documentElement.setAttribute("data-theme", resolve(currentPref));
  try {
    localStorage.setItem(STORAGE_KEY, currentPref);
  } catch {}
}

// Same as setAttr, but briefly enables a color crossfade (see .theme-anim in
// theme.css) so the switch is gradual. The class is removed after the
// transition so it never interferes with hover/interaction transitions.
function setAttrAnimated(pref) {
  if (resolve(pref) === resolve(currentPref)) {
    setAttr(pref); // no visible change (e.g. system→system) — skip the fade
    return;
  }
  // On Android the whole-page color crossfade animates hundreds of
  // elements at once and drops frames on low-end phones — it reads as
  // laggy. An instant swap is snappier there.
  if (document.documentElement.classList.contains("platform-android")) {
    setAttr(pref);
    return;
  }
  const root = document.documentElement;
  root.classList.add(ANIM_CLASS);
  setAttr(pref);
  if (animTimer) clearTimeout(animTimer);
  animTimer = setTimeout(() => root.classList.remove(ANIM_CLASS), ANIM_MS);
}

function onSystemChange() {
  if (currentPref === "system") setAttrAnimated("system");
}

/** Current stored preference ("dark" | "light" | "system"). */
export function currentTheme() {
  return currentPref;
}

/** Concrete theme actually showing ("dark" | "light"). */
export function resolvedTheme() {
  return resolve(currentPref);
}

/** Apply a preference with a gradual crossfade (instant, no round-trip). */
export function applyTheme(pref) {
  setAttrAnimated(pref);
}

/**
 * Wire up theming for the current window. Runs a synchronous best-guess
 * from the localStorage cache first (so there's no flash of the wrong
 * theme), then reconciles with the persisted config, and listens for
 * cross-window changes + OS theme changes.
 */
export function initTheme() {
  // Synchronous prime from cache — happens before any await.
  let cached = "light";
  try {
    cached = localStorage.getItem(STORAGE_KEY) || "light";
  } catch {}
  setAttr(cached);

  if (!listenersWired) {
    listenersWired = true;
    if (mq.addEventListener) mq.addEventListener("change", onSystemChange);
    else if (mq.addListener) mq.addListener(onSystemChange);
    listen("theme-changed", (e) => setAttrAnimated(e.payload || "light")).catch(() => {});
  }

  // Reconcile with the source of truth (config) in the background.
  invoke("get_config")
    .then((cfg) => setAttr(cfg?.theme || "light"))
    .catch(() => {});
}
