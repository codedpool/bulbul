import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

const AUTOSAVE_DELAY_MS = 600;

export default function ScratchpadView() {
  const [notes, setNotes] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [activeId, setActiveId] = useState(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [saveState, setSaveState] = useState("idle"); // idle | dirty | saving | saved
  const saveTimer = useRef(null);
  const dirtyRef = useRef(false);

  useEffect(() => {
    load();
    const un = listen("notes-changed", () => loadKeepSelection());
    return () => { un.then((f) => f()); };
  }, []);

  async function loadKeepSelection() {
    try {
      const rows = await invoke("list_notes");
      setNotes(rows);
    } catch (e) {
      console.error("notes refresh failed", e);
    }
  }

  async function load() {
    try {
      const rows = await invoke("list_notes");
      setNotes(rows);
      if (rows.length > 0 && !activeId) {
        openNote(rows[0]);
      }
    } catch (e) {
      console.error("notes load failed", e);
    } finally {
      setLoading(false);
    }
  }

  function openNote(note) {
    if (dirtyRef.current && activeId) {
      // flush current pending save before switching
      flushSave();
    }
    setActiveId(note.id);
    setTitle(note.title);
    setBody(note.body);
    setSaveState("idle");
    dirtyRef.current = false;
  }

  async function startNewNote() {
    if (dirtyRef.current && activeId) {
      await flushSave();
    }
    try {
      const note = await invoke("create_note", { title: "", body: "" });
      const next = [note, ...notes];
      setNotes(next);
      setActiveId(note.id);
      setTitle(note.title);
      setBody(note.body);
      setSaveState("idle");
      dirtyRef.current = false;
    } catch (e) {
      alert(String(e));
    }
  }

  async function removeNote(id) {
    if (!confirm("Delete this note?")) return;
    try {
      await invoke("delete_note", { id });
      const next = notes.filter((n) => n.id !== id);
      setNotes(next);
      if (activeId === id) {
        if (next.length > 0) openNote(next[0]);
        else { setActiveId(null); setTitle(""); setBody(""); }
      }
    } catch (e) {
      alert(String(e));
    }
  }

  // Debounced auto-save
  useEffect(() => {
    if (activeId == null) return;
    if (!dirtyRef.current) return;
    if (saveTimer.current) clearTimeout(saveTimer.current);
    setSaveState("dirty");
    saveTimer.current = setTimeout(() => {
      flushSave();
    }, AUTOSAVE_DELAY_MS);
    return () => {
      if (saveTimer.current) clearTimeout(saveTimer.current);
    };
  }, [title, body]);

  async function flushSave() {
    if (activeId == null) return;
    const id = activeId;
    const t = title;
    const b = body;
    setSaveState("saving");
    try {
      await invoke("update_note", { id, title: t, body: b });
      setSaveState("saved");
      setNotes((prev) => prev.map((n) => n.id === id ? { ...n, title: t, body: b, updated_at: Math.floor(Date.now() / 1000) } : n));
      dirtyRef.current = false;
      setTimeout(() => setSaveState((s) => (s === "saved" ? "idle" : s)), 1500);
    } catch (e) {
      setSaveState("idle");
      console.error("save failed", e);
    }
  }

  function onTitleChange(v) {
    dirtyRef.current = true;
    setTitle(v);
  }
  function onBodyChange(v) {
    dirtyRef.current = true;
    setBody(v);
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return notes;
    return notes.filter(
      (n) => n.title.toLowerCase().includes(q) || n.body.toLowerCase().includes(q),
    );
  }, [notes, search]);

  return (
    <div className="page scratchpad-page">
      <header className="page-header dictionary-header">
        <div>
          <h1>Scratchpad</h1>
          <p className="muted small">
            Drop a to-do list, polish a message before you send it, brain dump an idea. Notes save automatically.
          </p>
        </div>
        <button className="primary" onClick={startNewNote}>
          <PlusIcon /> New note
        </button>
      </header>

      <div className="scratchpad-layout">
        <aside className="scratch-sidebar">
          <div className="search-input scratch-search">
            <SearchIcon />
            <input
              type="text"
              placeholder="Search notes…"
              value={search}
              onChange={(e) => setSearch(e.target.value)}
              spellCheck={false}
            />
            {search && (
              <button className="clear-search" onClick={() => setSearch("")} aria-label="Clear search">×</button>
            )}
          </div>
          <div className="scratch-list">
            {loading ? (
              <div className="muted small list-empty">Loading…</div>
            ) : filtered.length === 0 ? (
              <div className="muted small list-empty">
                {search ? "No matches." : "No notes yet. Click \"New note\" to start."}
              </div>
            ) : (
              filtered.map((n) => (
                <NoteListItem
                  key={n.id}
                  note={n}
                  active={n.id === activeId}
                  onOpen={() => openNote(n)}
                  onDelete={() => removeNote(n.id)}
                />
              ))
            )}
          </div>
        </aside>

        <main className="scratch-editor">
          {activeId == null ? (
            <div className="scratch-empty">
              <h2>For quick thoughts you want to come back to</h2>
              <p className="muted">Click "New note" to start. Bulbul saves automatically.</p>
            </div>
          ) : (
            <>
              <div className="scratch-editor-toolbar">
                <input
                  type="text"
                  className="scratch-title"
                  placeholder="Untitled"
                  value={title}
                  onChange={(e) => onTitleChange(e.target.value)}
                  spellCheck={false}
                />
                <SaveBadge state={saveState} />
              </div>
              <textarea
                className="scratch-body"
                placeholder="Start typing, or dictate with your hotkey…"
                value={body}
                onChange={(e) => onBodyChange(e.target.value)}
              />
            </>
          )}
        </main>
      </div>
    </div>
  );
}

function NoteListItem({ note, active, onOpen, onDelete }) {
  const preview = (note.body || "").trim().split("\n")[0] || "";
  const displayTitle = note.title || "Untitled";
  return (
    <div className={`note-item ${active ? "active" : ""}`} onClick={onOpen}>
      <div className="note-item-row">
        <div className="note-item-title">{displayTitle}</div>
        <button
          className="icon-btn danger note-item-delete"
          onClick={(e) => { e.stopPropagation(); onDelete(); }}
          aria-label="Delete"
        >
          <TrashIcon />
        </button>
      </div>
      <div className="note-item-preview">{preview || "(no content)"}</div>
      <div className="note-item-time">{relativeTime(note.updated_at)}</div>
    </div>
  );
}

function SaveBadge({ state }) {
  let label = "";
  let cls = "save-badge";
  if (state === "dirty") { label = "Unsaved"; cls += " dirty"; }
  else if (state === "saving") { label = "Saving…"; cls += " saving"; }
  else if (state === "saved") { label = "Saved"; cls += " saved"; }
  if (!label) return null;
  return <span className={cls}>{label}</span>;
}

function relativeTime(unix) {
  const now = Math.floor(Date.now() / 1000);
  const diff = now - unix;
  if (diff < 60) return "just now";
  if (diff < 3600) return `${Math.floor(diff / 60)}m ago`;
  if (diff < 86400) return `${Math.floor(diff / 3600)}h ago`;
  if (diff < 7 * 86400) return `${Math.floor(diff / 86400)}d ago`;
  const d = new Date(unix * 1000);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
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

function TrashIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 6h18" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
      <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    </svg>
  );
}
