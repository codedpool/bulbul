// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

const has = (v) => typeof v === "string" && v.trim().length > 0;

// Mirrors Config::can_transcribe in src-tauri/src/config.rs: is the selected
// transcription provider configured well enough to try? Keep the two in sync.
export function canTranscribe(cfg) {
  switch (cfg?.stt_provider) {
    case "openai_compatible":
      return has(cfg.stt_endpoint_url) && has(cfg.stt_endpoint_model);
    case "local":
      return has(cfg.local_stt_model);
    default:
      return has(cfg?.groq_api_key);
  }
}

// Mirrors Config::can_clean_up.
export function canCleanUp(cfg) {
  if (cfg?.llm_provider === "openai_compatible") {
    return has(cfg.llm_endpoint_url) && has(cfg.llm_endpoint_model);
  }
  return has(cfg?.groq_api_key);
}

// Mirrors is_private_host in src-tauri/src/stt.rs: this machine or the user's
// own network, where sending a key over plain http is acceptable.
export function isPrivateHost(host) {
  const h = String(host || "").replace(/^\[|\]$/g, "").toLowerCase();
  if (!h) return false;
  if (h === "localhost" || h.endsWith(".localhost") || h.endsWith(".local")) return true;
  const v4 = h.match(/^(\d{1,3})\.(\d{1,3})\.(\d{1,3})\.(\d{1,3})$/);
  if (v4) {
    const [a, b] = [Number(v4[1]), Number(v4[2])];
    return a === 127 || a === 10 || (a === 172 && b >= 16 && b <= 31) || (a === 192 && b === 168) || (a === 169 && b === 254);
  }
  if (h.includes(":")) return h === "::1" || /^f[cd]/.test(h) || h.startsWith("fe80");
  return !h.includes(".");
}

// "localhost:11434" for display; null when the address isn't parseable yet.
export function endpointHost(url) {
  try {
    const u = new URL(String(url).trim());
    return u.host || null;
  } catch {
    return null;
  }
}

// Which radio a saved OpenAI-compatible endpoint belongs under.
export function endpointKind(url) {
  try {
    return isPrivateHost(new URL(String(url).trim()).hostname) ? "self" : "cloud";
  } catch {
    return "self";
  }
}

// One-line description of what's currently configured, for Settings and the
// wizard. Cleanup is reported separately because it can differ.
export function providerSummary(cfg) {
  let transcription = "Groq";
  if (cfg?.stt_provider === "openai_compatible") {
    const host = endpointHost(cfg.stt_endpoint_url) || "your server";
    transcription = endpointKind(cfg.stt_endpoint_url) === "self" ? `Your server (${host})` : host;
  } else if (cfg?.stt_provider === "local") {
    transcription = "On this computer";
  }

  let cleanup;
  if (cfg?.llm_provider === "openai_compatible") {
    const host = endpointHost(cfg.llm_endpoint_url) || "your server";
    cleanup = `Your server (${host})`;
  } else {
    cleanup = has(cfg?.groq_api_key) ? "Groq" : "Off (words are typed as spoken)";
  }
  return { transcription, cleanup };
}
