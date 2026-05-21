import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import FeatureHero from "../components/FeatureHero.jsx";

const SNIPPETS_HERO_SAMPLES = [
  { trigger: "my LinkedIn", expansion: "https://linkedin.com/in/john-doe/" },
  { trigger: "rewrite prompt", expansion: "Rewrite this to be more concise…" },
  { trigger: "intro email", expansion: "Hey — would love to chat later this week…" },
];

export default function SnippetsView() {
  const [entries, setEntries] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [adding, setAdding] = useState(false);
  const [editingId, setEditingId] = useState(null);

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      const rows = await invoke("list_snippets");
      setEntries(rows);
    } catch (e) {
      console.error("snippets load failed", e);
    } finally {
      setLoading(false);
    }
  }

  async function addEntry(payload) {
    await invoke("add_snippet", { trigger: payload.trigger, expansion: payload.expansion });
    setAdding(false);
    await load();
  }

  async function saveEdit(id, payload) {
    await invoke("update_snippet", { id, trigger: payload.trigger, expansion: payload.expansion });
    setEditingId(null);
    await load();
  }

  async function remove(id) {
    await invoke("delete_snippet", { id });
    await load();
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) =>
        e.trigger.toLowerCase().includes(q) ||
        e.expansion.toLowerCase().includes(q),
    );
  }, [entries, search]);

  return (
    <div className="page snippets-page">
      <header className="page-header dictionary-header">
        <div>
          <h1>Snippets</h1>
          <p className="muted small">
            Say a trigger phrase and Bulbul replaces it with the saved text — your email, an intro, a long prompt. Applied after dictionary substitutions.
          </p>
        </div>
        <button
          className="primary"
          onClick={() => { setEditingId(null); setAdding(true); }}
        >
          <PlusIcon /> Add new
        </button>
      </header>

      <FeatureHero
        dismissKey="bulbul.snippets.hero.dismissed"
        title={<>The stuff <em>you</em> shouldn't have to re-type.</>}
        samples={SNIPPETS_HERO_SAMPLES}
      />

      <div className="dict-toolbar">
        <div className="search-input">
          <SearchIcon />
          <input
            type="text"
            placeholder="Search triggers or expansions…"
            value={search}
            onChange={(e) => setSearch(e.target.value)}
            spellCheck={false}
          />
          {search && (
            <button className="clear-search" onClick={() => setSearch("")} aria-label="Clear search">
              ×
            </button>
          )}
        </div>
        <div className="dict-meta">
          {filtered.length} of {entries.length}
        </div>
      </div>

      <div className="dict-entries">
        {adding && (
          <SnippetForm
            initial={{ trigger: "", expansion: "" }}
            onSave={addEntry}
            onCancel={() => setAdding(false)}
          />
        )}

        {loading ? (
          <div className="empty-state"><p className="muted">Loading…</p></div>
        ) : filtered.length === 0 && !adding ? (
          <div className="empty-state">
            <p>{search ? "No matches." : "No snippets yet. Click \"Add new\" to save your first trigger."}</p>
            {!search && (
              <p className="muted small examples">
                Examples: <kbd>my email</kbd> → your address · <kbd>intro pitch</kbd> → an opener paragraph · <kbd>polish prompt</kbd> → an instruction
              </p>
            )}
          </div>
        ) : (
          filtered.map((e) => editingId === e.id ? (
            <SnippetForm
              key={e.id}
              initial={e}
              onSave={(payload) => saveEdit(e.id, payload)}
              onCancel={() => setEditingId(null)}
            />
          ) : (
            <SnippetRow
              key={e.id}
              entry={e}
              onEdit={() => { setAdding(false); setEditingId(e.id); }}
              onDelete={() => remove(e.id)}
            />
          ))
        )}
      </div>
    </div>
  );
}

function SnippetRow({ entry, onEdit, onDelete }) {
  const oneLine = entry.expansion.replace(/\s+/g, " ").trim();
  const preview = oneLine.length > 100 ? oneLine.slice(0, 100) + "…" : oneLine;
  return (
    <div className="snippet-row">
      <div className="snippet-row-main">
        <div className="snippet-trigger-line">
          <span className="snippet-trigger">{entry.trigger}</span>
          <span className="dict-arrow">→</span>
          <span className="snippet-expansion">{preview}</span>
        </div>
      </div>
      <div className="dict-row-meta">
        <span className="dict-hits">{entry.hit_count} {entry.hit_count === 1 ? "use" : "uses"}</span>
        <div className="dict-row-actions">
          <button className="icon-btn" onClick={onEdit} aria-label="Edit"><EditIcon /></button>
          <button className="icon-btn danger" onClick={onDelete} aria-label="Delete"><TrashIcon /></button>
        </div>
      </div>
    </div>
  );
}

function SnippetForm({ initial, onSave, onCancel }) {
  const [trigger, setTrigger] = useState(initial.trigger || "");
  const [expansion, setExpansion] = useState(initial.expansion || "");
  const [error, setError] = useState("");

  async function submit() {
    const t = trigger.trim();
    const e = expansion.trim();
    if (!t) { setError("Trigger is required."); return; }
    if (!e) { setError("Expansion is required."); return; }
    try {
      await onSave({ trigger: t, expansion: e });
    } catch (err) {
      setError(String(err));
    }
  }

  function onKey(e) {
    if (e.key === "Enter" && (e.ctrlKey || e.metaKey)) { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
  }

  return (
    <div className="snippet-form">
      <div className="snippet-form-row">
        <label>Trigger</label>
        <input
          type="text"
          placeholder='e.g. "my email", "intro pitch", "polish prompt"'
          value={trigger}
          onChange={(e) => setTrigger(e.target.value)}
          onKeyDown={onKey}
          autoFocus
          spellCheck={false}
        />
      </div>
      <div className="snippet-form-row">
        <label>Expansion</label>
        <textarea
          placeholder="The text Bulbul should insert when you say the trigger."
          value={expansion}
          onChange={(e) => setExpansion(e.target.value)}
          onKeyDown={onKey}
          rows={5}
        />
      </div>
      <div className="dict-form-actions">
        <span className="muted small">Tip: <kbd>Ctrl</kbd> + <kbd>Enter</kbd> to save</span>
        <div className="spacer" />
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" onClick={submit} disabled={!trigger.trim() || !expansion.trim()}>Save</button>
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

function SearchIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <circle cx="11" cy="11" r="8" />
      <line x1="21" y1="21" x2="16.65" y2="16.65" />
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
