// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

// Shared state for on-device models: what's installed, what's downloading and
// what went wrong. One store for the whole window, so a download started in
// the setup wizard keeps reporting after that screen is gone.

import { useSyncExternalStore } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { IS_ANDROID } from "./platform.js";

// The speech engine reports through the same events, under this id.
export const ENGINE_ID = "engine";

let state = {
  loaded: false,
  models: [],
  engineInstalled: null, // null until first answer
  progress: {}, // id -> { phase, done, total }
  errors: {}, // id -> { kind, message, needed_bytes, available_bytes }
};
const listeners = new Set();
let started = false;

function set(patch) {
  state = { ...state, ...patch };
  listeners.forEach((l) => l());
}

function without(obj, key) {
  if (!(key in obj)) return obj;
  const { [key]: _gone, ...rest } = obj;
  return rest;
}

export async function refreshLocalModels() {
  if (IS_ANDROID) return;
  try {
    const [models, engine] = await Promise.all([invoke("list_local_models"), invoke("local_engine_status")]);
    set({ models, engineInstalled: engine.installed, loaded: true });
  } catch (e) {
    set({ loaded: true });
  }
}

// Starts listening once. Safe to call from every consumer.
export function startLocalModels() {
  if (started || IS_ANDROID) return;
  started = true;
  listen("local-model-progress", (e) => {
    const p = e.payload;
    set({
      progress: { ...state.progress, [p.model_id]: { phase: p.phase, done: p.bytes_done, total: p.bytes_total } },
      errors: without(state.errors, p.model_id),
    });
  });
  listen("local-model-done", (e) => {
    const id = e.payload;
    set({ progress: without(state.progress, id), errors: without(state.errors, id) });
    refreshLocalModels();
  });
  listen("local-model-error", (e) => {
    const err = e.payload;
    const progress = without(state.progress, err.model_id);
    // Pausing is the user's own action, not a failure to report.
    set({ progress, errors: err.kind === "cancelled" ? state.errors : { ...state.errors, [err.model_id]: err } });
    refreshLocalModels();
  });
  refreshLocalModels();
}

// Downloads the speech engine first when it isn't installed yet (both run at
// once; the model can finish before the engine does).
export async function downloadLocalModel(id) {
  set({ errors: without(without(state.errors, id), ENGINE_ID) });
  if (!state.engineInstalled && !state.progress[ENGINE_ID]) {
    try {
      await invoke("install_local_engine");
    } catch (e) {
      set({ errors: { ...state.errors, [ENGINE_ID]: { kind: "other", message: String(e) } } });
    }
  }
  try {
    await invoke("download_local_model", { id });
  } catch (e) {
    set({ errors: { ...state.errors, [id]: { kind: "other", message: String(e) } } });
  }
  refreshLocalModels();
}

// Starts the download unless the model is already there or on its way.
export async function ensureLocalModel(id) {
  await refreshLocalModels();
  const m = state.models.find((x) => x.id === id);
  if (!m || m.status === "installed" || m.downloading || state.progress[id]) {
    // The model may be ready while the engine still isn't (a fresh install
    // that kept its models folder).
    if (m && !state.engineInstalled && !state.progress[ENGINE_ID]) await retryEngineInstall();
    return;
  }
  await downloadLocalModel(id);
}

export async function retryEngineInstall() {
  set({ errors: without(state.errors, ENGINE_ID) });
  try {
    await invoke("install_local_engine");
  } catch (e) {
    set({ errors: { ...state.errors, [ENGINE_ID]: { kind: "other", message: String(e) } } });
  }
}

export async function pauseLocalModel(id) {
  try {
    await invoke("cancel_local_model_download", { id });
  } catch {
    /* nothing to pause */
  }
}

export async function deleteLocalModel(id) {
  try {
    await invoke("delete_local_model", { id });
  } catch (e) {
    set({ errors: { ...state.errors, [id]: { kind: "other", message: String(e) } } });
  }
  refreshLocalModels();
}

function subscribe(fn) {
  listeners.add(fn);
  return () => listeners.delete(fn);
}

export function useLocalModels() {
  startLocalModels();
  return useSyncExternalStore(subscribe, () => state);
}

// ---------- formatting shared by the cards, the picker and the pill ----------

export function formatBytes(n) {
  if (!n) return "0 MB";
  if (n >= 1e9) return `${(n / 1e9).toFixed(n >= 1e10 ? 0 : 1)} GB`;
  return `${Math.max(1, Math.round(n / 1e6))} MB`;
}

export function percent(p) {
  if (!p || !p.total) return 0;
  return Math.min(100, Math.floor((p.done / p.total) * 100));
}

// The step a download is in, in words.
export function phaseLabel(p) {
  switch (p?.phase) {
    case "verifying":
      return "Checking the download";
    case "unpacking":
      return "Unpacking";
    default:
      return "Downloading";
  }
}

// What to tell someone whose download stopped, and what they can do.
export function describeError(err) {
  switch (err?.kind) {
    case "no_space":
      return `Not enough free disk space. It needs ${formatBytes(err.needed_bytes)} and ${formatBytes(err.available_bytes)} is free.`;
    case "network":
      return "The connection dropped. What's downloaded so far is kept, so you can resume.";
    case "corrupt":
      return "The download didn't pass its safety check and was discarded. Try again.";
    default:
      return err?.message || "Something went wrong.";
  }
}

// The model a first-time setup should offer, given the dictation language.
export function suggestedModelId(language) {
  const lang = String(language || "").toLowerCase();
  return !lang || lang === "en" || lang === "auto" ? "parakeet-tdt-110m-en" : "whisper-small";
}
