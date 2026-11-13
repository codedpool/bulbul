// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useLayoutEffect, useRef, useState } from "react";
import { createPortal } from "react-dom";
import { invoke } from "@tauri-apps/api/core";
import { endpointKind, isPrivateHost } from "../provider.js";
import { IS_ANDROID } from "../platform.js";
import { ensureLocalModel, suggestedModelId } from "../localModels.js";
import { LocalModelPicker } from "./LocalModels.jsx";
import "./ProviderSetup.css";

// Where the data goes is the real difference between the options, so every
// one says it in plain words.
const OPENAI_PRESET = { url: "https://api.openai.com/v1", model: "whisper-1" };
const OPENAI_CHAT_PRESET = { url: "https://api.openai.com/v1", model: "gpt-4o-mini" };

const icon = {
  cloud: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M17.5 19a4.5 4.5 0 1 0-1.4-8.78A6 6 0 1 0 6 18.5" />
      <path d="M6 19h11.5" />
    </svg>
  ),
  network: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="3" y="4" width="18" height="6" rx="1.5" />
      <rect x="3" y="14" width="18" height="6" rx="1.5" />
      <path d="M7 7h.01M7 17h.01" />
    </svg>
  ),
  chip: (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="6" y="6" width="12" height="12" rx="2" />
      <path d="M9 2v3M15 2v3M9 19v3M15 19v3M2 9h3M2 15h3M19 9h3M19 15h3" />
    </svg>
  ),
};

const IDLE = { status: "idle", message: "" };

// Said once, plainly: a speech model only does the first step.
const INTRO = "Transcription turns your voice into text. Cleanup polishes it, and is a separate step. Groq is the default for both.";

function Field({ id, label, hint, extra, children }) {
  return (
    <div className="ps-field">
      <div className="ps-field-head">
        <label htmlFor={id}>
          {label}
          {hint && <span className="ps-field-hint">{hint}</span>}
        </label>
        {extra}
      </div>
      {children}
    </div>
  );
}

// The button lives beside the "Server address" label; the result only takes
// room when there is one.
function TestButton({ state, onTest, disabled }) {
  return (
    <button
      type="button"
      className="ps-chip"
      onClick={onTest}
      disabled={disabled || state.status === "checking"}
    >
      {state.status === "checking" ? "Testing…" : "Test connection"}
    </button>
  );
}

function TestStatus({ state }) {
  if (state.status === "ok") return <p className="ps-test-status ps-ok" role="status">Connected</p>;
  if (state.status === "error") return <p className="ps-test-status ps-err" role="alert">{state.message}</p>;
  return null;
}

// ---------- state shared by the modal and the Settings page ----------

function useProviderDraft(config, updateConfig) {
  const [choice, setChoice] = useState("groq"); // groq | self | cloud | local
  const [localModel, setLocalModel] = useState("");
  const [stt, setStt] = useState({ url: "", model: "", key: "" });
  const [llmChoice, setLlmChoice] = useState("groq"); // groq | self | cloud
  const [llm, setLlm] = useState({ url: "", model: "", key: "" });
  const [fallback, setFallback] = useState(true);
  const [sttTest, setSttTest] = useState(IDLE);
  const [llmTest, setLlmTest] = useState(IDLE);
  const [error, setError] = useState("");
  const [saving, setSaving] = useState(false);
  const [saved, setSaved] = useState(false);

  const hasGroqKey = !!(config?.groq_api_key || "").trim();

  // Load the saved settings. Called when the modal opens / the page mounts.
  function load() {
    const c = config || {};
    setChoice(
      c.stt_provider === "openai_compatible" ? endpointKind(c.stt_endpoint_url) : c.stt_provider === "local" && !IS_ANDROID ? "local" : "groq"
    );
    setLocalModel(c.local_stt_model || suggestedModelId(c.language));
    setStt({ url: c.stt_endpoint_url || "", model: c.stt_endpoint_model || "", key: c.stt_endpoint_key || "" });
    setLlmChoice(c.llm_provider === "openai_compatible" ? endpointKind(c.llm_endpoint_url) : "groq");
    setLlm({ url: c.llm_endpoint_url || "", model: c.llm_endpoint_model || "", key: c.llm_endpoint_key || "" });
    setFallback(c.stt_cloud_fallback !== false);
    setSttTest(IDLE);
    setLlmTest(IDLE);
    setError("");
    setSaved(false);
  }

  function touch() {
    setError("");
    setSaved(false);
  }

  function pickChoice(next) {
    setChoice(next);
    setSttTest(IDLE);
    touch();
  }
  function pickLlm(next) {
    setLlmChoice(next);
    setLlmTest(IDLE);
    touch();
  }
  function editStt(patch) {
    setStt((s) => ({ ...s, ...patch }));
    setSttTest(IDLE);
    touch();
  }
  function editLlm(patch) {
    setLlm((s) => ({ ...s, ...patch }));
    setLlmTest(IDLE);
    touch();
  }
  function pickLocalModel(id) {
    setLocalModel(id);
    touch();
  }
  function editFallback(v) {
    setFallback(v);
    touch();
  }

  async function runTest(cmd, values, setState) {
    setState({ status: "checking", message: "" });
    try {
      await invoke(cmd, { url: values.url, model: values.model, key: values.key });
      setState({ status: "ok", message: "" });
    } catch (e) {
      setState({ status: "error", message: String(e) });
    }
  }

  // The plain-http-with-a-key rule is enforced by the backend too; catching
  // it here just saves a round trip.
  function addressProblem(v, needKey) {
    if (!v.url.trim()) return "Enter the server address.";
    if (!/^https?:\/\//i.test(v.url.trim())) return "The address needs to start with http:// or https://";
    if (!v.model.trim()) return "Enter the model name.";
    if (needKey && !v.key.trim()) return "This provider needs an API key.";
    try {
      const u = new URL(v.url.trim());
      if (v.key.trim() && u.protocol === "http:" && !isPrivateHost(u.hostname)) {
        return "Your key would be sent unencrypted. Use https:// for servers outside your own network.";
      }
    } catch {
      return "That address isn't a valid URL.";
    }
    return "";
  }

  // Resolves true when saved.
  async function save() {
    let problem = "";
    if (choice === "local") problem = localModel ? "" : "Pick a model to download.";
    else if (choice !== "groq") problem = addressProblem(stt, choice === "cloud");
    if (!problem && llmChoice !== "groq") problem = addressProblem(llm, llmChoice === "cloud");
    if (problem) {
      setError(problem);
      return false;
    }
    setSaving(true);
    try {
      const patch = {
        stt_provider: choice === "groq" ? "groq" : choice === "local" ? "local" : "openai_compatible",
        stt_cloud_fallback: fallback,
        llm_provider: llmChoice === "groq" ? "groq" : "openai_compatible",
      };
      if (choice === "local") patch.local_stt_model = localModel;
      if (choice === "self" || choice === "cloud") {
        patch.stt_endpoint_url = stt.url.trim();
        patch.stt_endpoint_model = stt.model.trim();
        patch.stt_endpoint_key = stt.key.trim();
      }
      if (llmChoice !== "groq") {
        patch.llm_endpoint_url = llm.url.trim();
        patch.llm_endpoint_model = llm.model.trim();
        patch.llm_endpoint_key = llm.key.trim();
      }
      await updateConfig({ ...config, ...patch });
      // Start fetching in the background; the progress pill shows where it is.
      if (choice === "local") ensureLocalModel(localModel);
      setSaved(true);
      return true;
    } catch (e) {
      setError(String(e));
      return false;
    } finally {
      setSaving(false);
    }
  }

  return {
    choice, localModel, stt, llmChoice, llm, fallback, sttTest, llmTest, error, saving, saved, hasGroqKey,
    load, pickChoice, pickLocalModel, pickLlm, editStt, editLlm, editFallback, save,
    testStt: () => runTest("test_stt_endpoint", stt, setSttTest),
    testLlm: () => runTest("test_llm_endpoint", llm, setLlmTest),
  };
}

// ---------- the form itself ----------

function Option({ name, value, checked, onChange, title, hint, tag, desc, where, disabled, children, showDesc }) {
  return (
    <div className={`ps-opt ${checked ? "selected" : ""} ${disabled ? "disabled" : ""}`}>
      <label className="ps-opt-main">
        <input type="radio" name={name} value={value} checked={checked} disabled={disabled} onChange={onChange} />
        <span className="ps-radio" aria-hidden />
        <span className="ps-opt-text">
          <span className="ps-opt-title">
            {title}
            {tag && <span className="ps-tag">{tag}</span>}
            {hint && <span className="ps-opt-hint">{hint}</span>}
          </span>
          {desc && showDesc && <span className="ps-opt-desc">{desc}</span>}
          <span className="ps-where">
            {icon[where.icon]}
            {where.text}
          </span>
        </span>
      </label>
      {checked && children}
    </div>
  );
}

function ProviderForm({ d, variant }) {
  const pane = variant === "pane";
  const { choice, stt, llmChoice, llm, fallback } = d;
  const noCleanup = llmChoice === "groq" && !d.hasGroqKey;

  const sttForm = (
    <div className="ps-form">
      <Field
        id="ps-stt-url"
        label="Server address"
        extra={
          <span className="ps-field-actions">
            {choice === "cloud" && (
              <button
                type="button"
                className="ps-chip"
                onClick={() => d.editStt({ url: OPENAI_PRESET.url, model: OPENAI_PRESET.model })}
              >
                Use OpenAI
              </button>
            )}
            <TestButton state={d.sttTest} disabled={!stt.url.trim() || !stt.model.trim()} onTest={d.testStt} />
          </span>
        }
      >
        <input
          id="ps-stt-url"
          type="text"
          value={stt.url}
          placeholder={choice === "cloud" ? "https://api.openai.com/v1" : "http://localhost:11434/v1"}
          onChange={(e) => d.editStt({ url: e.target.value })}
          spellCheck={false}
          autoComplete="off"
        />
      </Field>
      <div className="ps-row">
        <Field id="ps-stt-model" label="Model">
          <input
            id="ps-stt-model"
            type="text"
            value={stt.model}
            placeholder="whisper-1"
            onChange={(e) => d.editStt({ model: e.target.value })}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        <Field id="ps-stt-key" label="API key" hint={choice === "cloud" ? "" : "optional"}>
          <input
            id="ps-stt-key"
            type="password"
            value={stt.key}
            placeholder={choice === "cloud" ? "sk-…" : "If your server asks"}
            onChange={(e) => d.editStt({ key: e.target.value })}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      </div>
      <TestStatus state={d.sttTest} />
    </div>
  );

  const llmForm = (
    <div className="ps-form">
      <Field
        id="ps-llm-url"
        label="Server address"
        extra={
          <span className="ps-field-actions">
            {llmChoice === "cloud" && (
              <button
                type="button"
                className="ps-chip"
                onClick={() => d.editLlm({ url: OPENAI_CHAT_PRESET.url, model: OPENAI_CHAT_PRESET.model })}
              >
                Use OpenAI
              </button>
            )}
            <TestButton state={d.llmTest} disabled={!llm.url.trim() || !llm.model.trim()} onTest={d.testLlm} />
          </span>
        }
      >
        <input
          id="ps-llm-url"
          type="text"
          value={llm.url}
          placeholder={llmChoice === "cloud" ? "https://api.openai.com/v1" : "http://localhost:1234/v1"}
          onChange={(e) => d.editLlm({ url: e.target.value })}
          spellCheck={false}
          autoComplete="off"
        />
      </Field>
      <div className="ps-row">
        <Field id="ps-llm-model" label="Model">
          <input
            id="ps-llm-model"
            type="text"
            value={llm.model}
            placeholder={llmChoice === "cloud" ? "gpt-4o-mini" : "qwen2.5-7b-instruct"}
            onChange={(e) => d.editLlm({ model: e.target.value })}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
        <Field id="ps-llm-key" label="API key" hint={llmChoice === "cloud" ? "" : "optional"}>
          <input
            id="ps-llm-key"
            type="password"
            value={llm.key}
            placeholder={llmChoice === "cloud" ? "sk-…" : "If your server asks"}
            onChange={(e) => d.editLlm({ key: e.target.value })}
            spellCheck={false}
            autoComplete="off"
          />
        </Field>
      </div>
      <TestStatus state={d.llmTest} />
    </div>
  );


  const fallbackToggle =
    choice !== "groq" ? (
      <label className="toggle-row ps-fallback">
        <span className="toggle-text">
          <span className="toggle-label">If transcription fails, retry on Groq</span>
          <span className="toggle-hint">
            {d.hasGroqKey
              ? "Sends that recording to Groq, and tells you."
              : "Needs a Groq key; without one a failure shows an error."}
          </span>
        </span>
        <span className={`toggle ${fallback ? "on" : ""}`}>
          <input type="checkbox" checked={fallback} onChange={(e) => d.editFallback(e.target.checked)} />
          <span className="toggle-thumb" />
        </span>
      </label>
    ) : null;

  return (
    <div className={`ps-cols ${pane ? "ps-cols-pane" : ""}`}>
      <section className="ps-col" aria-labelledby="ps-stt-title">
        <h3 id="ps-stt-title">Transcription</h3>
        <p className="ps-col-lede">Turns your voice into text.</p>
        <div className="ps-group" role="radiogroup" aria-label="Transcription provider">
          <Option
            name="ps-choice" value="groq" checked={choice === "groq"} onChange={() => d.pickChoice("groq")}
            title="Groq" tag="Default" showDesc={pane || choice === "groq"}
            desc={d.hasGroqKey ? "Fast, and free with your own key. Using the key you've already added." : "Fast, and free with your own key. Add the key on the previous screen or in Settings."}
            where={{ icon: "cloud", text: "Audio is sent to Groq." }}
          />
          <Option
            name="ps-choice" value="self" checked={choice === "self"} onChange={() => d.pickChoice("self")}
            title="A server I run" hint="Ollama, whisper.cpp…" showDesc={pane}
            desc="Any server on your network that speaks the OpenAI transcription API."
            where={{ icon: "network", text: "Audio goes only to the address you enter." }}
          >
            {sttForm}
          </Option>
          <Option
            name="ps-choice" value="cloud" checked={choice === "cloud"} onChange={() => d.pickChoice("cloud")}
            title="Another cloud provider" hint="OpenAI…" showDesc={pane}
            desc="Any service with an OpenAI-style transcription API."
            where={{ icon: "cloud", text: "Audio is sent to that provider." }}
          >
            {sttForm}
          </Option>
          {!IS_ANDROID && (
            <Option
              name="ps-choice" value="local" checked={choice === "local"} onChange={() => d.pickChoice("local")}
              title="A model on this computer" hint="Works offline" showDesc={pane}
              desc="Download a speech model once and transcribe without the internet."
              where={{ icon: "chip", text: "Audio never leaves this computer. Downloads when you save." }}
            >
              <LocalModelPicker selected={d.localModel} onSelect={d.pickLocalModel} />
            </Option>
          )}
        </div>

        {pane && fallbackToggle}
      </section>

      <section className="ps-col" aria-labelledby="ps-clean-title">
        <h3 id="ps-clean-title">Cleanup</h3>
        <p className="ps-col-lede">Punctuation, filler words and polish. Needs a language model.</p>
        <div className="ps-group" role="radiogroup" aria-label="Cleanup provider">
          <Option
            name="ps-llm" value="groq" checked={llmChoice === "groq"} onChange={() => d.pickLlm("groq")}
            title="Groq" showDesc={pane}
            desc="Uses your Groq key."
            where={{ icon: "cloud", text: "Your transcript text is sent to Groq." }}
          />
          <Option
            name="ps-llm" value="self" checked={llmChoice === "self"} onChange={() => d.pickLlm("self")}
            title="A server I run" hint="Ollama, LM Studio…" showDesc={pane}
            desc="Any OpenAI-compatible chat server."
            where={{ icon: "network", text: "Text goes only to the address you enter." }}
          >
            {llmForm}
          </Option>
          <Option
            name="ps-llm" value="cloud" checked={llmChoice === "cloud"} onChange={() => d.pickLlm("cloud")}
            title="Another cloud provider" hint="OpenAI, OpenRouter…" showDesc={pane}
            desc="Any service with an OpenAI-style chat API."
            where={{ icon: "cloud", text: "Your transcript text is sent to that provider." }}
          >
            {llmForm}
          </Option>
        </div>
        {!pane && fallbackToggle}
        {noCleanup && (
          <p className="ps-note ps-note-warn">
            No cleanup service is set, so Bulbul will type your words exactly as you said them.
          </p>
        )}
      </section>
    </div>
  );
}

// ---------- modal (setup wizard) ----------

export default function ProviderSetupModal({ open, config, updateConfig, onClose }) {
  const d = useProviderDraft(config, updateConfig);
  const cardRef = useRef(null);

  // Layout effect so the first painted frame already shows the saved choice.
  useLayoutEffect(() => {
    if (open) d.load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [open]);

  // Escape closes; Tab stays inside the dialog.
  useEffect(() => {
    if (!open) return;
    const onKey = (e) => {
      if (e.key === "Escape") {
        e.preventDefault();
        onClose();
      } else if (e.key === "Tab" && cardRef.current) {
        const nodes = cardRef.current.querySelectorAll(
          'button:not(:disabled), input:not(:disabled), a[href], [tabindex]:not([tabindex="-1"])'
        );
        if (!nodes.length) return;
        const first = nodes[0];
        const last = nodes[nodes.length - 1];
        if (e.shiftKey && document.activeElement === first) {
          e.preventDefault();
          last.focus();
        } else if (!e.shiftKey && document.activeElement === last) {
          e.preventDefault();
          first.focus();
        }
      }
    };
    document.addEventListener("keydown", onKey);
    // One frame later, once the saved choice has been rendered as checked.
    const raf = requestAnimationFrame(() => {
      cardRef.current?.querySelector('input[type="radio"]:checked')?.focus();
    });
    return () => {
      cancelAnimationFrame(raf);
      document.removeEventListener("keydown", onKey);
    };
  }, [open, onClose]);

  if (!open) return null;

  return createPortal(
    <div
      className="ps-overlay"
      onMouseDown={(e) => {
        if (e.target === e.currentTarget) onClose();
      }}
    >
      <div className="ps-card" role="dialog" aria-modal="true" aria-labelledby="ps-title" ref={cardRef}>
        <header className="ps-head">
          <h2 id="ps-title">Transcription and cleanup</h2>
          <p>{INTRO}</p>
        </header>

        <div className="ps-body">
          <ProviderForm d={d} variant="modal" />
        </div>

        <footer className="ps-foot">
          {d.error && <span className="ps-err ps-foot-err" role="alert">{d.error}</span>}
          <button type="button" onClick={onClose}>Cancel</button>
          <button
            type="button"
            className="primary"
            disabled={d.saving}
            onClick={async () => {
              if (await d.save()) onClose();
            }}
          >
            {d.saving ? "Saving…" : "Save"}
          </button>
        </footer>
      </div>
    </div>,
    document.body
  );
}

// ---------- Settings > Providers page ----------

export function ProviderSetupPane({ config, updateConfig }) {
  const d = useProviderDraft(config, updateConfig);

  useLayoutEffect(() => {
    d.load();
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  return (
    <div className="ps-pane">
      <p className="ps-pane-lede">{INTRO}</p>
      <ProviderForm d={d} variant="pane" />
      <div className="ps-pane-foot">
        <button type="button" className="primary" disabled={d.saving} onClick={() => d.save()}>
          {d.saving ? "Saving…" : "Save changes"}
        </button>
        {d.saved && <span className="ps-ok" role="status">Saved</span>}
        {d.error && <span className="ps-err" role="alert">{d.error}</span>}
      </div>
    </div>
  );
}
