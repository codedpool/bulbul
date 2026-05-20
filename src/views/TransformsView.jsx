import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export default function TransformsView() {
  const [transforms, setTransforms] = useState([]);
  const [loading, setLoading] = useState(true);
  const [editing, setEditing] = useState(null); // null | { id?, name, description, system_prompt }

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      const rows = await invoke("list_transforms");
      setTransforms(rows);
    } catch (e) {
      console.error("transforms load failed", e);
    } finally {
      setLoading(false);
    }
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

  async function remove(id) {
    if (!confirm("Delete this transform?")) return;
    await invoke("delete_transform", { id });
    if (editing?.id === id) setEditing(null);
    await load();
  }

  async function setDefault(id) {
    await invoke("set_default_transform", { id });
    await load();
  }

  async function resetAll() {
    if (!confirm("Reset to default transforms? This deletes any custom ones.")) return;
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
            Apply an AI transform to selected text — Bulbul rewrites, cleans up, or restructures it in place. The default transform runs when you press your polish hotkey or click the wand on the pill.
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
          {transforms.map((t, idx) => (
            <TransformCard
              key={t.id}
              transform={t}
              slot={idx < 9 ? idx + 1 : null}
              onEdit={() => setEditing(t)}
              onDelete={() => remove(t.id)}
              onSetDefault={() => setDefault(t.id)}
            />
          ))}
        </div>
      )}
    </div>
  );
}

function TransformCard({ transform, slot, onEdit, onDelete, onSetDefault }) {
  return (
    <div className={`transform-card ${transform.is_default ? "default" : ""}`}>
      <div className="transform-card-top">
        <span />{/* per-transform hotkey badge deferred — only default runs via polish hotkey */}
        <div className="transform-card-actions">
          {transform.is_default ? (
            <span className="default-pill">Default</span>
          ) : (
            <button
              className="text-btn small"
              onClick={onSetDefault}
              title="Use when polish hotkey or wand is triggered"
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
