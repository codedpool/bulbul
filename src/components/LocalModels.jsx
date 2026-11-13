// SPDX-License-Identifier: GPL-3.0-only
// Copyright (c) 2026 Romanch Roshan Singh

import { useEffect, useState } from "react";
import { createPortal } from "react-dom";
import ConfirmDialog from "./ConfirmDialog.jsx";
import {
  ENGINE_ID,
  deleteLocalModel,
  describeError,
  downloadLocalModel,
  formatBytes,
  pauseLocalModel,
  percent,
  phaseLabel,
  retryEngineInstall,
  useLocalModels,
} from "../localModels.js";
import "./LocalModels.css";

function languagesOf(m) {
  if (m.multilingual) return m.languages.length ? m.languages.join(", ") : "Many languages";
  return "English";
}

function Bar({ value, label }) {
  return (
    <div className="lm-bar" role="progressbar" aria-valuemin={0} aria-valuemax={100} aria-valuenow={value} aria-label={label}>
      <div className="lm-bar-fill" style={{ width: `${value}%` }} />
    </div>
  );
}

// What a card (or picker row) should show for one model right now.
function modelState(m, store) {
  const progress = store.progress[m.id];
  if (progress || m.downloading) return { kind: "downloading", progress };
  if (m.status === "installed") return { kind: "installed" };
  if (m.status === "incomplete") return { kind: "paused" };
  return { kind: "idle" };
}

// ---------- Settings > Local models ----------

function EngineRow({ store }) {
  const progress = store.progress[ENGINE_ID];
  const err = store.errors[ENGINE_ID];
  let text;
  if (progress) text = `${phaseLabel(progress)} the speech engine… ${percent(progress)}%`;
  else if (store.engineInstalled) text = "Installed. It runs models in a separate process, so a crash can't take Bulbul down.";
  else text = "Downloaded once, about 7 MB, the first time you get a model.";
  return (
    <div className="lm-engine">
      <div className="lm-engine-main">
        <span className="lm-engine-title">Speech engine</span>
        <span className="lm-engine-text">{text}</span>
        {err && <span className="lm-err" role="alert">{describeError(err)}</span>}
      </div>
      {!store.engineInstalled && !progress && (
        <button type="button" className="lm-btn" onClick={retryEngineInstall}>
          {err ? "Try again" : "Download now"}
        </button>
      )}
    </div>
  );
}

function ModelCard({ m, store, inUse, onUse, onDelete }) {
  const st = modelState(m, store);
  const err = store.errors[m.id];
  const pct = percent(st.progress);
  const paused = st.kind === "paused";
  const partial = m.partial_bytes;

  return (
    <li className={`lm-card ${inUse ? "in-use" : ""}`}>
      <div className="lm-card-top">
        <div className="lm-card-title">
          <h4>{m.name}</h4>
          {m.experimental && <span className="lm-tag lm-tag-warn">Experimental</span>}
          {inUse && <span className="lm-tag lm-tag-accent">In use</span>}
          {st.kind === "installed" && !inUse && <span className="lm-tag">Ready</span>}
        </div>
        <div className="lm-actions">
          {st.kind === "idle" && (
            <button type="button" className="lm-btn primary" onClick={() => downloadLocalModel(m.id)}>
              Download
            </button>
          )}
          {st.kind === "downloading" && (
            <button type="button" className="lm-btn" onClick={() => pauseLocalModel(m.id)}>
              Pause
            </button>
          )}
          {paused && (
            <>
              <button type="button" className="lm-btn primary" onClick={() => downloadLocalModel(m.id)}>
                Resume
              </button>
              <button type="button" className="lm-btn lm-btn-quiet" onClick={() => onDelete(m)}>
                Discard
              </button>
            </>
          )}
          {st.kind === "installed" && (
            <>
              {!inUse && (
                <button type="button" className="lm-btn primary" onClick={() => onUse(m)}>
                  Use this
                </button>
              )}
              <button type="button" className="lm-btn lm-btn-quiet" onClick={() => onDelete(m)}>
                Delete
              </button>
            </>
          )}
        </div>
      </div>

      <p className="lm-summary">{m.summary}</p>
      <p className="lm-facts">
        <span>{languagesOf(m)}</span>
        <span>{formatBytes(m.download_bytes)} download</span>
        <span>{formatBytes(m.disk_bytes)} on disk</span>
        <span>{m.license.spdx}</span>
      </p>

      {st.kind === "downloading" && (
        <div className="lm-progress">
          <Bar value={pct} label={`${m.name} download`} />
          <span className="lm-progress-text">
            {phaseLabel(st.progress)}
            {st.progress?.total ? ` · ${formatBytes(st.progress.done)} of ${formatBytes(st.progress.total)}` : "…"}
          </span>
        </div>
      )}
      {paused && (
        <p className="lm-paused">
          Paused{partial ? ` with ${formatBytes(partial)} already on this computer` : ""}. Resuming carries on from there.
        </p>
      )}
      {err && <p className="lm-err" role="alert">{describeError(err)}</p>}
      {m.license.attribution && <p className="lm-credit">{m.license.attribution}</p>}
      {st.kind === "installed" && !m.verified && (
        <p className="lm-credit">These files were added by hand, so Bulbul hasn't checked them.</p>
      )}
    </li>
  );
}

export function LocalModelsPane({ config, updateConfig }) {
  const store = useLocalModels();
  const [toDelete, setToDelete] = useState(null);

  async function use(m) {
    await updateConfig({ ...config, stt_provider: "local", local_stt_model: m.id });
  }

  async function remove(m) {
    // Deleting the model in use leaves nothing to transcribe with locally.
    if (config.stt_provider === "local" && config.local_stt_model === m.id) {
      await updateConfig({ ...config, stt_provider: "groq" });
    }
    await deleteLocalModel(m.id);
  }

  return (
    <div className="lm-pane">
      <p className="lm-lede">
        Download a speech model once and Bulbul can turn your voice into text without the internet. Your audio never
        leaves this computer. Cleanup is a separate step and still needs a language model, set under Providers.
      </p>
      <EngineRow store={store} />
      {!store.loaded ? (
        <p className="lm-empty">Loading…</p>
      ) : store.models.length === 0 ? (
        <p className="lm-empty">No models are available on this platform.</p>
      ) : (
        <ul className="lm-list">
          {store.models.map((m) => (
            <ModelCard
              key={m.id}
              m={m}
              store={store}
              inUse={config.stt_provider === "local" && config.local_stt_model === m.id}
              onUse={use}
              onDelete={(model) => (model.status === "installed" ? setToDelete(model) : remove(model))}
            />
          ))}
        </ul>
      )}
      <ConfirmDialog
        open={!!toDelete}
        title={toDelete ? `Delete ${toDelete.name}?` : ""}
        message={
          toDelete && config.stt_provider === "local" && config.local_stt_model === toDelete.id
            ? "It's the model you dictate with. Bulbul will switch to Groq until you pick another. You can download it again later."
            : "Frees the disk space. You can download it again later."
        }
        confirmLabel="Delete"
        danger
        onConfirm={() => {
          const m = toDelete;
          setToDelete(null);
          remove(m);
        }}
        onCancel={() => setToDelete(null)}
      />
    </div>
  );
}

// ---------- the model list inside the provider setup modal ----------

export function LocalModelPicker({ selected, onSelect }) {
  const store = useLocalModels();
  if (!store.loaded) return <p className="lm-empty">Loading models…</p>;
  if (!store.models.length) return <p className="lm-empty">No models are available on this platform.</p>;
  return (
    <>
    <div className="lm-pick" role="radiogroup" aria-label="On-device model">
      {store.models.map((m) => {
        const st = modelState(m, store);
        const checked = selected === m.id;
        return (
          <label key={m.id} className={`lm-pick-row ${checked ? "selected" : ""}`}>
            <input type="radio" name="lm-pick" value={m.id} checked={checked} onChange={() => onSelect(m.id)} />
            <span className="lm-pick-dot" aria-hidden />
            <span className="lm-pick-text">
              <span className="lm-pick-name">
                {m.name}
                {m.experimental && <span className="lm-tag lm-tag-warn">Experimental</span>}
              </span>
            </span>
            <span className="lm-pick-size">
              {st.kind === "installed" ? "Ready" : st.kind === "paused" ? "Partly downloaded" : formatBytes(m.download_bytes)}
            </span>
          </label>
        );
      })}
    </div>
    <p className="lm-pick-desc">{store.models.find((m) => m.id === selected)?.summary}</p>
    </>
  );
}

// ---------- download progress, anywhere in the app ----------

// Floats over the app while anything is downloading, or after one fails, so
// a download started in the setup wizard is never out of sight.
export function DownloadPill() {
  const store = useLocalModels();
  const active = Object.keys(store.progress);
  const failed = Object.keys(store.errors).filter((id) => store.errors[id].kind !== "cancelled");
  const [dismissed, setDismissed] = useState({});

  // A fresh failure for something previously dismissed should show again.
  useEffect(() => {
    setDismissed((d) => {
      const next = { ...d };
      for (const id of Object.keys(next)) if (!store.errors[id]) delete next[id];
      return next;
    });
  }, [store.errors]);

  const nameOf = (id) => (id === ENGINE_ID ? "the speech engine" : store.models.find((m) => m.id === id)?.name || id);

  if (active.length) {
    // Prefer the model over the 7 MB engine: that's the part people wait for.
    const id = active.find((a) => a !== ENGINE_ID) || active[0];
    const p = store.progress[id];
    return createPortal(
      <div className="lm-pill" role="status">
        <div className="lm-pill-text">
          <span className="lm-pill-title">
            {phaseLabel(p)} {nameOf(id)}
          </span>
          <span className="lm-pill-sub">{p.total ? `${percent(p)}% of ${formatBytes(p.total)}` : "Starting…"}</span>
        </div>
        <Bar value={percent(p)} label="Download progress" />
        {id !== ENGINE_ID && (
          <button type="button" className="lm-btn lm-btn-quiet" onClick={() => pauseLocalModel(id)}>
            Pause
          </button>
        )}
      </div>,
      document.body
    );
  }

  const shown = failed.find((id) => !dismissed[id]);
  if (shown) {
    return createPortal(
      <div className="lm-pill lm-pill-error" role="alert">
        <div className="lm-pill-text">
          <span className="lm-pill-title">Couldn't download {nameOf(shown)}</span>
          <span className="lm-pill-sub">{describeError(store.errors[shown])}</span>
        </div>
        <div className="lm-pill-actions">
          <button
            type="button"
            className="lm-btn primary"
            onClick={() => (shown === ENGINE_ID ? retryEngineInstall() : downloadLocalModel(shown))}
          >
            Try again
          </button>
          <button type="button" className="lm-btn lm-btn-quiet" onClick={() => setDismissed((d) => ({ ...d, [shown]: true }))}>
            Dismiss
          </button>
        </div>
      </div>,
      document.body
    );
  }
  return null;
}

// ---------- a half-finished download, noticed at startup ----------

export function InterruptedDownloadModal({ config, updateConfig, onChooseAnother }) {
  const store = useLocalModels();
  const [closed, setClosed] = useState(false);

  const id = config?.stt_provider === "local" ? config.local_stt_model : "";
  const model = store.models.find((m) => m.id === id);
  const downloading = !!store.progress[id] || !!model?.downloading;
  const open = !closed && !!model && !downloading && model.status !== "installed";
  const hasGroqKey = !!(config?.groq_api_key || "").trim();

  useEffect(() => {
    if (!open) return;
    const onKey = (e) => e.key === "Escape" && setClosed(true);
    document.addEventListener("keydown", onKey);
    return () => document.removeEventListener("keydown", onKey);
  }, [open]);

  if (!open) return null;
  const partial = model.status === "incomplete" ? model.partial_bytes : 0;

  return createPortal(
    <div className="lm-overlay" onMouseDown={(e) => e.target === e.currentTarget && setClosed(true)}>
      <div className="lm-dialog" role="dialog" aria-modal="true" aria-labelledby="lm-int-title">
        <h2 id="lm-int-title">{model.name} isn't finished downloading</h2>
        <p>
          {partial
            ? `${formatBytes(partial)} of ${formatBytes(model.download_bytes)} is already on this computer, so resuming is quick.`
            : `It needs a ${formatBytes(model.download_bytes)} download before it can transcribe.`}{" "}
          Until it's ready, {hasGroqKey && config.stt_cloud_fallback !== false ? "Bulbul will use Groq instead." : "dictation will show an error."}
        </p>
        <div className="lm-dialog-actions">
          <button
            type="button"
            className="lm-btn primary"
            onClick={() => {
              setClosed(true);
              downloadLocalModel(model.id);
            }}
          >
            {partial ? "Resume download" : "Download"}
          </button>
          <button
            type="button"
            className="lm-btn"
            onClick={() => {
              setClosed(true);
              onChooseAnother?.();
            }}
          >
            Choose another model
          </button>
          <button
            type="button"
            className="lm-btn lm-btn-quiet"
            disabled={!hasGroqKey}
            title={hasGroqKey ? "" : "Add a Groq key in Settings first"}
            onClick={async () => {
              setClosed(true);
              await updateConfig({ ...config, stt_provider: "groq" });
            }}
          >
            Use Groq instead
          </button>
        </div>
      </div>
    </div>,
    document.body
  );
}
