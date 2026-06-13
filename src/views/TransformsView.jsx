import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import FeatureHero from "../components/FeatureHero.jsx";
import ConfirmDialog from "../components/ConfirmDialog.jsx";
import { IS_MAC } from "../platform.js";

const TRANSFORMS_HERO_SAMPLES = [
  { trigger: "Polish", expansion: "Fix grammar, tighten flow, keep meaning." },
  { trigger: "Make formal", expansion: "Rewrite in a professional register." },
  { trigger: "Bullet points", expansion: "Restructure prose as a clean list." },
];

export default function TransformsView() {
  const [transforms, setTransforms] = useState([]);
  const [slotStatuses, setSlotStatuses] = useState([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState(null); // null | { id?, name, description, system_prompt }

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      const [rows, statuses] = await Promise.all([
        invoke("list_transforms"),
        invoke("list_transform_slot_statuses"),
      ]);
      setTransforms(rows);
      setSlotStatuses(statuses || []);
    } catch (e) {
      console.error("transforms load failed", e);
    } finally {
      setLoading(false);
    }
  }

  function statusForTransform(id) {
    return slotStatuses.find((s) => s.transform_id === id) || null;
  }

  async function saveTransform(payload) {
    if (payload.id) {
      await invoke("update_transform", {
        id: payload.id,
        name: payload.name,
        description: payload.description,
        systemPrompt: payload.system_prompt,
      });
    } else {
      await invoke("add_transform", {
        name: payload.name,
        description: payload.description,
        systemPrompt: payload.system_prompt,
      });
    }
    setEditing(null);
    await load();
  }

  // Dialog state: each pending confirmation drives a themed ConfirmDialog.
  const [pendingDeleteId, setPendingDeleteId] = useState(null);
  const [confirmingReset, setConfirmingReset] = useState(false);

  function remove(id) {
    setPendingDeleteId(id);
  }

  async function confirmDelete() {
    const id = pendingDeleteId;
    setPendingDeleteId(null);
    if (id == null) return;
    await invoke("delete_transform", { id });
    if (editing?.id === id) setEditing(null);
    await load();
  }

  async function setDefault(id) {
    await invoke("set_default_transform", { id });
    await load();
  }

  function resetAll() {
    setConfirmingReset(true);
  }

  async function confirmReset() {
    setConfirmingReset(false);
    await invoke("reset_transforms");
    setEditing(null);
    await load();
  }

  return (
    <div className="page transforms-page">
      <header className="page-header dictionary-header">
        <div>
          <h1>Transforms</h1>
          <p className="muted small">
            Select text anywhere, press {IS_MAC ? "⌘1 through ⌘6" : "Alt+1 through Alt+6"}, and Bulbul rewrites it — polish, retone, or restructure. The one marked default is what Bulbul uses everywhere else.
          </p>
        </div>
        <div className="header-actions">
          <button onClick={resetAll} title="Reset to default transforms">
            <ResetIcon /> Reset to defaults
          </button>
          <button
            className="primary"
            onClick={() => setEditing({ name: "", description: "", system_prompt: "" })}
          >
            <PlusIcon /> Create new
          </button>
        </div>
      </header>

      <FeatureHero
        dismissKey="bulbul.transforms.hero.dismissed"
        title={<>Rewrite anything you write — with <em>one hotkey.</em></>}
        samples={TRANSFORMS_HERO_SAMPLES}
      />

      <BindingFailureBanner
        slotStatuses={slotStatuses}
        transforms={transforms}
        loading={loading}
      />

      {editing && (
        <TransformEditor
          initial={editing}
          onSave={saveTransform}
          onCancel={() => setEditing(null)}
        />
      )}

      {loading ? (
        <div className="empty-state"><p className="muted">Loading…</p></div>
      ) : transforms.length === 0 ? (
        <div className="empty-state">
          <p>No transforms yet. Click "Create new" or "Reset to defaults".</p>
        </div>
      ) : (
        <div className="transform-grid">
          {transforms.map((t) => (
            <TransformCard
              key={t.id}
              transform={t}
              slotStatus={statusForTransform(t.id)}
              onEdit={() => setEditing(t)}
              onDelete={() => remove(t.id)}
              onSetDefault={() => setDefault(t.id)}
            />
          ))}
        </div>
      )}

      <ConfirmDialog
        open={pendingDeleteId !== null}
        title="Delete this transform?"
        message="This can't be undone."
        confirmLabel="Delete"
        danger
        onConfirm={confirmDelete}
        onCancel={() => setPendingDeleteId(null)}
      />
      <ConfirmDialog
        open={confirmingReset}
        title="Reset to default transforms?"
        message="This deletes any custom transforms you've added. Defaults will be restored."
        confirmLabel="Reset"
        danger
        onConfirm={confirmReset}
        onCancel={() => setConfirmingReset(false)}
      />
    </div>
  );
}

// Translate Windows' raw RegisterHotKey error text into something a
// non-technical user can act on. Falls back to the raw error if we can't
// recognise it (better than silently swallowing).
function humanizeBindingError(raw) {
  if (!raw) return "another app on your computer is already using this combo";
  const s = raw.toLowerCase();
  if (s.includes("already") || s.includes("1409")) {
    return "another app is already using this combo";
  }
  if (s.includes("access") || s.includes("denied")) {
    return "Windows denied the registration (try restarting Bulbul as the same user)";
  }
  if (s.includes("not representable")) {
    return "this combo isn't a Windows-valid shortcut";
  }
  return raw;
}

function BindingFailureBanner({ slotStatuses, transforms, loading }) {
  if (loading) return null;
  const blocked = slotStatuses.filter((s) => s && !s.registered);
  if (blocked.length === 0) return null;
  const transformById = new Map(transforms.map((t) => [t.id, t]));
  return (
    <div className="transforms-binding-banner" role="status">
      <div className="transforms-binding-banner-head">
        <svg
          width="18"
          height="18"
          viewBox="0 0 24 24"
          fill="none"
          stroke="currentColor"
          strokeWidth="2"
          strokeLinecap="round"
          strokeLinejoin="round"
          aria-hidden
          className="transforms-binding-banner-icon"
        >
          <path d="M10.29 3.86 1.82 18a2 2 0 0 0 1.71 3h16.94a2 2 0 0 0 1.71-3L13.71 3.86a2 2 0 0 0-3.42 0z" />
          <line x1="12" y1="9" x2="12" y2="13" />
          <line x1="12" y1="17" x2="12.01" y2="17" />
        </svg>
        <div>
          <strong>
            {blocked.length === 1
              ? "1 transform shortcut couldn't be claimed"
              : `${blocked.length} transform shortcuts couldn't be claimed`}
          </strong>
          <p className="muted small">
            Windows only lets one app own each global hotkey, and another
            app on your computer registered first. The transforms below
            still work — you can still trigger them by clicking their
            card — the keyboard shortcut just won't fire until the
            conflict is resolved.
          </p>
        </div>
      </div>
      <ul className="transforms-binding-banner-list">
        {blocked.map((s) => {
          const t = transformById.get(s.transform_id);
          return (
            <li key={s.transform_id}>
              <code className="transforms-binding-combo">{s.combo}</code>
              {t && (
                <span className="transforms-binding-name">{t.name}</span>
              )}
              <span className="transforms-binding-reason">
                — {humanizeBindingError(s.error)}
              </span>
            </li>
          );
        })}
      </ul>
      <div className="transforms-binding-banner-hint muted small">
        Common culprits: OBS, Steam, AutoHotkey scripts, screenshot tools,
        and launcher apps. Close or reconfigure the offender, then click{" "}
        <em>Reset to defaults</em> above to retry.
      </div>
    </div>
  );
}

function TransformCard({ transform, slotStatus, onEdit, onDelete, onSetDefault }) {
  return (
    <div className={`transform-card ${transform.is_default ? "default" : ""}`}>
      <div className="transform-card-top">
        {slotStatus ? (
          <span
            className={`slot-chip ${slotStatus.registered ? "ok" : "blocked"}`}
            title={
              slotStatus.registered
                ? `Press ${slotStatus.combo} to run this transform on selected text`
                : `${slotStatus.combo} is unavailable — another app already owns this combo${slotStatus.error ? ` (${slotStatus.error})` : ""}`
            }
          >
            {slotStatus.combo}
            {!slotStatus.registered && <span className="slot-chip-x" aria-hidden>×</span>}
          </span>
        ) : (
          <span />
        )}
        <div className="transform-card-actions">
          {transform.is_default ? (
            <span className="default-pill">Default</span>
          ) : (
            <button
              className="text-btn small"
              onClick={onSetDefault}
              title="Mark this as your default transform. The slot binding (Alt+N) stays its own thing."
            >
              Set default
            </button>
          )}
        </div>
      </div>
      <div className="transform-card-body" onClick={onEdit}>
        <h3 className="transform-name">{transform.name}</h3>
        {transform.description && (
          <p className="transform-desc">{transform.description}</p>
        )}
        <p className="transform-hits muted small">
          {transform.hit_count > 0 ? `Used ${transform.hit_count} ${transform.hit_count === 1 ? "time" : "times"}` : "Not used yet"}
        </p>
      </div>
      <div className="transform-card-bottom">
        <button className="icon-btn" onClick={onEdit} aria-label="Edit"><EditIcon /></button>
        <button className="icon-btn danger" onClick={onDelete} aria-label="Delete"><TrashIcon /></button>
      </div>
    </div>
  );
}

function TransformEditor({ initial, onSave, onCancel }) {
  const [name, setName] = useState(initial.name || "");
  const [description, setDescription] = useState(initial.description || "");
  const [prompt, setPrompt] = useState(initial.system_prompt || "");
  const [error, setError] = useState("");

  async function submit() {
    const n = name.trim();
    const p = prompt.trim();
    if (!n) { setError("Name is required."); return; }
    if (!p) { setError("System prompt is required."); return; }
    try {
      await onSave({ id: initial.id, name: n, description: description.trim(), system_prompt: p });
    } catch (e) {
      setError(String(e));
    }
  }

  function onKey(e) {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
  }

  return (
    <div className="transform-editor">
      <div className="snippet-form-row">
        <label>Name</label>
        <input
          type="text"
          placeholder='e.g. "Bulletize", "Translate to Hindi"'
          value={name}
          onChange={(e) => setName(e.target.value)}
          onKeyDown={onKey}
          autoFocus
          spellCheck={false}
        />
      </div>
      <div className="snippet-form-row">
        <label>Description</label>
        <input
          type="text"
          placeholder="One-line description shown on the card"
          value={description}
          onChange={(e) => setDescription(e.target.value)}
          onKeyDown={onKey}
          spellCheck={false}
        />
      </div>
      <div className="snippet-form-row">
        <label>System prompt</label>
        <textarea
          placeholder="Tell the LLM how to rewrite the user's text. Be specific. End with 'Return ONLY the rewritten text.'"
          value={prompt}
          onChange={(e) => setPrompt(e.target.value)}
          onKeyDown={onKey}
          rows={8}
        />
      </div>
      <div className="dict-form-actions">
        <span className="muted small">Tip: <kbd>Ctrl</kbd> + <kbd>Enter</kbd> to save</span>
        <div className="spacer" />
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" onClick={submit} disabled={!name.trim() || !prompt.trim()}>
          {initial.id ? "Save" : "Create"}
        </button>
      </div>
      {error && <p className="err">{error}</p>}
    </div>
  );
}

function PlusIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}

function ResetIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 12a9 9 0 1 0 3-6.7" />
      <polyline points="3 4 3 10 9 10" />
    </svg>
  );
}

function EditIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" />
    </svg>
  );
}

function TrashIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 6h18" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
      <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    </svg>
  );
}
