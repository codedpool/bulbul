import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

export default function DictionaryView() {
  const [entries, setEntries] = useState([]);
  const [loading, setLoading] = useState(true);
  const [draftFrom, setDraftFrom] = useState("");
  const [draftTo, setDraftTo] = useState("");
  const [draftCase, setDraftCase] = useState(false);
  const [draftError, setDraftError] = useState("");
  const [editingId, setEditingId] = useState(null);
  const [editFrom, setEditFrom] = useState("");
  const [editTo, setEditTo] = useState("");
  const [editCase, setEditCase] = useState(false);

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      const rows = await invoke("list_dictionary");
      setEntries(rows);
    } catch (e) {
      console.error("dictionary load failed", e);
    } finally {
      setLoading(false);
    }
  }

  async function add() {
    setDraftError("");
    const from = draftFrom.trim();
    const to = draftTo.trim();
    if (!from || !to) {
      setDraftError("Both fields are required.");
      return;
    }
    try {
      await invoke("add_dictionary_entry", {
        fromWord: from,
        toWord: to,
        caseSensitive: draftCase,
      });
      setDraftFrom("");
      setDraftTo("");
      setDraftCase(false);
      await load();
    } catch (e) {
      setDraftError(String(e));
    }
  }

  function startEdit(entry) {
    setEditingId(entry.id);
    setEditFrom(entry.from_word);
    setEditTo(entry.to_word);
    setEditCase(entry.case_sensitive);
  }

  function cancelEdit() {
    setEditingId(null);
  }

  async function saveEdit() {
    try {
      await invoke("update_dictionary_entry", {
        id: editingId,
        fromWord: editFrom,
        toWord: editTo,
        caseSensitive: editCase,
      });
      setEditingId(null);
      await load();
    } catch (e) {
      alert(String(e));
    }
  }

  async function remove(id) {
    try {
      await invoke("delete_dictionary_entry", { id });
      await load();
    } catch (e) {
      alert(String(e));
    }
  }

  return (
    <div className="page dictionary-page">
      <header className="page-header">
        <h1>Dictionary</h1>
        <p className="muted small">
          Replace transcribed words automatically. Applied after Groq cleanup, before injection. Case-insensitive by default.
        </p>
      </header>

      <section className="add-entry">
        <div className="add-entry-row">
          <input
            type="text"
            placeholder="when I say..."
            value={draftFrom}
            onChange={(e) => { setDraftFrom(e.target.value); setDraftError(""); }}
            onKeyDown={(e) => { if (e.key === "Enter") add(); }}
            spellCheck={false}
          />
          <span className="arrow">→</span>
          <input
            type="text"
            placeholder="...write it like this"
            value={draftTo}
            onChange={(e) => { setDraftTo(e.target.value); setDraftError(""); }}
            onKeyDown={(e) => { if (e.key === "Enter") add(); }}
            spellCheck={false}
          />
          <label className="case-toggle" title="Match case exactly">
            <input
              type="checkbox"
              checked={draftCase}
              onChange={(e) => setDraftCase(e.target.checked)}
            />
            <span>Aa</span>
          </label>
          <button className="primary" onClick={add} disabled={!draftFrom.trim() || !draftTo.trim()}>
            Add
          </button>
        </div>
        {draftError && <p className="err">{draftError}</p>}
      </section>

      <section className="entries-section">
        <div className="entries-header">
          <h3>Entries</h3>
          <span className="muted small">
            {entries.length} {entries.length === 1 ? "entry" : "entries"}
          </span>
        </div>

        {loading ? (
          <div className="empty-state"><p className="muted">Loading…</p></div>
        ) : entries.length === 0 ? (
          <div className="empty-state">
            <p>No dictionary entries yet. Add one above.</p>
          </div>
        ) : (
          <div className="entries">
            {entries.map((e) => editingId === e.id ? (
              <div key={e.id} className="entry-row editing">
                <input
                  type="text"
                  value={editFrom}
                  onChange={(ev) => setEditFrom(ev.target.value)}
                  onKeyDown={(ev) => { if (ev.key === "Enter") saveEdit(); if (ev.key === "Escape") cancelEdit(); }}
                  autoFocus
                />
                <span className="arrow">→</span>
                <input
                  type="text"
                  value={editTo}
                  onChange={(ev) => setEditTo(ev.target.value)}
                  onKeyDown={(ev) => { if (ev.key === "Enter") saveEdit(); if (ev.key === "Escape") cancelEdit(); }}
                />
                <label className="case-toggle" title="Match case exactly">
                  <input
                    type="checkbox"
                    checked={editCase}
                    onChange={(ev) => setEditCase(ev.target.checked)}
                  />
                  <span>Aa</span>
                </label>
                <div className="entry-actions">
                  <button className="primary small-btn" onClick={saveEdit}>Save</button>
                  <button className="small-btn" onClick={cancelEdit}>Cancel</button>
                </div>
              </div>
            ) : (
              <div key={e.id} className="entry-row">
                <span className="entry-from">{e.from_word}</span>
                <span className="arrow">→</span>
                <span className="entry-to">{e.to_word}</span>
                <span className="entry-meta">
                  {e.case_sensitive && <span className="badge muted-badge">case</span>}
                  <span className="hits">{e.hit_count} {e.hit_count === 1 ? "use" : "uses"}</span>
                </span>
                <div className="entry-actions">
                  <button className="icon-btn" onClick={() => startEdit(e)} title="Edit">
                    <EditIcon />
                  </button>
                  <button className="icon-btn danger" onClick={() => remove(e.id)} title="Delete">
                    <TrashIcon />
                  </button>
                </div>
              </div>
            ))}
          </div>
        )}
      </section>
    </div>
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
