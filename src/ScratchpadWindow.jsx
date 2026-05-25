import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import "./ScratchpadWindow.css";

const AUTOSAVE_DELAY_MS = 600;

export default function ScratchpadWindow() {
  const [notes, setNotes] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [activeId, setActiveId] = useState(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [saveState, setSaveState] = useState("idle");
  const [sidebarOpen, setSidebarOpen] = useState(true);
  const [copyState, setCopyState] = useState("idle");
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
      if (rows.length > 0) openNote(rows[0]);
    } catch (e) {
      console.error("notes load failed", e);
    } finally {
      setLoading(false);
    }
  }

  function openNote(note) {
    if (dirtyRef.current && activeId) flushSave();
    setActiveId(note.id);
    setTitle(note.title);
    setBody(note.body);
    setSaveState("idle");
    dirtyRef.current = false;
  }

  async function startNewNote() {
    if (dirtyRef.current && activeId) await flushSave();
    try {
      const note = await invoke("create_note", { title: "", body: "" });
      setNotes((prev) => [note, ...prev]);
      setActiveId(note.id);
      setTitle("");
      setBody("");
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

  useEffect(() => {
    if (activeId == null) return;
    if (!dirtyRef.current) return;
    if (saveTimer.current) clearTimeout(saveTimer.current);
    setSaveState("dirty");
    saveTimer.current = setTimeout(flushSave, AUTOSAVE_DELAY_MS);
    return () => { if (saveTimer.current) clearTimeout(saveTimer.current); };
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
      setTimeout(() => setSaveState((s) => (s === "saved" ? "idle" : s)), 1400);
    } catch (e) {
      setSaveState("idle");
      console.error("save failed", e);
    }
  }

  async function copyBody() {
    try {
      await navigator.clipboard.writeText(body);
      setCopyState("copied");
      setTimeout(() => setCopyState("idle"), 1500);
    } catch (e) {
      console.error("copy failed", e);
    }
  }

  function onTitleChange(v) { dirtyRef.current = true; setTitle(v); }
  function onBodyChange(v) { dirtyRef.current = true; setBody(v); }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return notes;
    return notes.filter(
      (n) => n.title.toLowerCase().includes(q) || n.body.toLowerCase().includes(q),
    );
  }, [notes, search]);

  const displayTitle = title || "Untitled";

  return (
    <div className={`sp-shell ${sidebarOpen ? "sidebar-open" : "sidebar-closed"}`}>
      <SpTitleBar />
      <div className="sp-body">
        {sidebarOpen && (
          <aside className="sp-sidebar">
            <button className="sp-side-btn" onClick={() => setSidebarOpen(false)}>
              <SidebarIcon />
              <span>Collapse Notes</span>
            </button>
            <button className="sp-side-btn" onClick={startNewNote}>
              <NewNoteIcon />
              <span>New note</span>
            </button>
            <div className="sp-side-search">
              <SearchIcon />
              <input
                type="text"
                placeholder="Search notes…"
                value={search}
                onChange={(e) => setSearch(e.target.value)}
                spellCheck={false}
              />
            </div>
            <div className="sp-side-sep" />
            <div className="sp-note-list">
              {loading ? (
                <div className="sp-side-empty">Loading…</div>
              ) : filtered.length === 0 ? (
                <div className="sp-side-empty">{search ? "No matches." : "No notes yet"}</div>
              ) : (
                filtered.map((n) => (
                  <button
                    key={n.id}
                    className={`sp-note-item ${n.id === activeId ? "active" : ""}`}
                    onClick={() => openNote(n)}
                  >
                    <div className="sp-note-item-title">{n.title || "Untitled"}</div>
                    <div className="sp-note-item-preview">
                      {(n.body || "").trim().split("\n")[0] || "(empty)"}
                    </div>
                    <button
                      className="sp-note-item-trash"
                      onClick={(e) => { e.stopPropagation(); removeNote(n.id); }}
                      aria-label="Delete"
                    >
                      <TrashIcon />
                    </button>
                  </button>
                ))
              )}
            </div>
          </aside>
        )}

        {!sidebarOpen && (
          <button className="sp-sidebar-handle" onClick={() => setSidebarOpen(true)} title="Show notes">
            <SidebarIcon />
          </button>
        )}

        <main className="sp-editor">
          {activeId == null ? (
            <div className="sp-empty">
              <h2>For quick thoughts you want to come back to</h2>
              <p>Click "New note" to start. Bulbul saves automatically.</p>
              <button className="sp-cta" onClick={startNewNote}>
                Start new note
              </button>
            </div>
          ) : (
            <>
              <div className="sp-editor-head">
                <input
                  type="text"
                  className="sp-title"
                  placeholder="Untitled"
                  value={title}
                  onChange={(e) => onTitleChange(e.target.value)}
                  spellCheck={false}
                />
                <SaveBadge state={saveState} />
              </div>
              <textarea
                className="sp-body-input"
                placeholder="Start typing, or dictate with your hotkey…"
                value={body}
                onChange={(e) => onBodyChange(e.target.value)}
              />
              <button className="sp-copy" onClick={copyBody} disabled={!body}>
                <CopyIcon />
                {copyState === "copied" ? "Copied" : "Copy"}
              </button>
            </>
          )}
        </main>
      </div>
    </div>
  );
}

function SpTitleBar() {
  const win = getCurrentWindow();
  return (
    <div className="sp-titlebar" data-tauri-drag-region>
      <div className="sp-titlebar-spacer" data-tauri-drag-region />
      <div className="sp-titlebar-controls">
        <button
          className="sp-tb-btn"
          aria-label="Minimize"
          title="Minimize"
          onClick={() => win.minimize().catch(() => {})}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <line x1="1.5" y1="5" x2="8.5" y2="5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
          </svg>
        </button>
        <button
          className="sp-tb-btn sp-tb-close"
          aria-label="Close"
          title="Close"
          onClick={() => win.close().catch(() => {})}
        >
          <svg width="10" height="10" viewBox="0 0 10 10" aria-hidden>
            <line x1="1.5" y1="1.5" x2="8.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
            <line x1="8.5" y1="1.5" x2="1.5" y2="8.5" stroke="currentColor" strokeWidth="1" strokeLinecap="round" />
          </svg>
        </button>
      </div>
    </div>
  );
}

function SaveBadge({ state }) {
  let label = "";
  let cls = "sp-save";
  if (state === "dirty") { label = "Unsaved"; cls += " dirty"; }
  else if (state === "saving") { label = "Saving…"; cls += " saving"; }
  else if (state === "saved") { label = "Saved"; cls += " saved"; }
  if (!label) return null;
  return <span className={cls}>{label}</span>;
}

function PlusIcon() {
  return (
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2.5" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="12" y1="5" x2="12" y2="19" />
      <line x1="5" y1="12" x2="19" y2="12" />
    </svg>
  );
}
function SidebarIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="3" y="3" width="18" height="18" rx="2" />
      <line x1="9" y1="3" x2="9" y2="21" />
    </svg>
  );
}
function NewNoteIcon() {
  return (
    <svg width="16" height="16" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M12 20h9" />
      <path d="M16.5 3.5a2.121 2.121 0 0 1 3 3L7 19l-4 1 1-4 12.5-12.5z" />
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
    <svg width="12" height="12" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <path d="M3 6h18" />
      <path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" />
      <path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" />
    </svg>
  );
}
function CopyIcon() {
  return (
    <svg width="13" height="13" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <rect x="9" y="9" width="13" height="13" rx="2" />
      <path d="M5 15H4a2 2 0 0 1-2-2V4a2 2 0 0 1 2-2h9a2 2 0 0 1 2 2v1" />
    </svg>
  );
}
