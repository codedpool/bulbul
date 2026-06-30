import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { getCurrentWindow } from "@tauri-apps/api/window";
import ConfirmDialog from "./components/ConfirmDialog.jsx";
import TooltipProvider from "./components/TooltipProvider.jsx";
import { IS_MAC } from "./platform.js";
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
  const [transforms, setTransforms] = useState([]);
  const [runningTransformId, setRunningTransformId] = useState(null);
  const [transformError, setTransformError] = useState("");
  const [hasSelection, setHasSelection] = useState(false);
  const saveTimer = useRef(null);
  const dirtyRef = useRef(false);
  const bodyRef = useRef(null);
  const selRef = useRef({ start: 0, end: 0 });

  useEffect(() => {
    load();
    invoke("list_transforms").then(setTransforms).catch(() => {});
    const un = listen("notes-changed", () => loadKeepSelection());
    return () => { un.then((f) => f()); };
  }, []);

  // Dictation into the scratchpad: when the scratchpad window is the
  // focused webview at inject-time, the orchestrator (lib.rs) emits
  // `scratchpad-append` with the cleaned transcript instead of going
  // through the OS-level Cmd+V / Ctrl+V path. Routing the OS paste
  // back into our own window was fragile on macOS (System Events
  // keystroke routing depends on TCC permission + focus order); a
  // direct IPC insert sidesteps that entirely and also doesn't touch
  // the user's clipboard.
  //
  // Insert at the textarea's current cursor (or replace the current
  // selection). Falls back to appending at the end if the textarea
  // somehow doesn't have a usable selection.
  useEffect(() => {
    const un = listen("scratchpad-append", (event) => {
      const incoming = String(event.payload || "");
      if (!incoming) return;
      const el = bodyRef.current;
      // Prefer the live caret position from the textarea itself; if it
      // isn't focused right now, fall back to the last remembered
      // selection (selRef), and finally to appending at end.
      let start;
      let end;
      if (el && document.activeElement === el) {
        start = el.selectionStart;
        end = el.selectionEnd;
      } else if (selRef.current && Number.isFinite(selRef.current.start)) {
        start = selRef.current.start;
        end = selRef.current.end;
      } else {
        start = body.length;
        end = body.length;
      }
      setBody((prev) => {
        const next = prev.slice(0, start) + incoming + prev.slice(end);
        dirtyRef.current = true;
        return next;
      });
      requestAnimationFrame(() => {
        const node = bodyRef.current;
        if (!node) return;
        const caret = start + incoming.length;
        node.focus();
        node.setSelectionRange(caret, caret);
        selRef.current = { start: caret, end: caret };
      });
    });
    return () => { un.then((f) => f()); };
  }, [body]);

  function rememberSelection() {
    const el = bodyRef.current;
    if (!el) return;
    selRef.current = { start: el.selectionStart, end: el.selectionEnd };
    setHasSelection(el.selectionEnd > el.selectionStart);
  }

  async function applyTransform(transform) {
    const { start, end } = selRef.current;
    if (end <= start) return;
    const selected = body.slice(start, end);
    setRunningTransformId(transform.id);
    setTransformError("");
    try {
      const out = await invoke("run_transform_on_text", {
        transformId: transform.id,
        text: selected,
      });
      const next = body.slice(0, start) + out + body.slice(end);
      onBodyChange(next);
      requestAnimationFrame(() => {
        const el = bodyRef.current;
        if (el) {
          const caret = start + out.length;
          el.focus();
          el.setSelectionRange(caret, caret);
          selRef.current = { start: caret, end: caret };
        }
      });
    } catch (e) {
      setTransformError(String(e));
    } finally {
      setRunningTransformId(null);
    }
  }

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
      setErrorMsg(String(e));
    }
  }

  const [pendingDelete, setPendingDelete] = useState(null);
  const [errorMsg, setErrorMsg] = useState(null);

  function removeNote(id) {
    setPendingDelete(id);
  }

  async function confirmDelete() {
    const id = pendingDelete;
    setPendingDelete(null);
    if (id == null) return;
    try {
      await invoke("delete_note", { id });
      const next = notes.filter((n) => n.id !== id);
      setNotes(next);
      if (activeId === id) {
        if (next.length > 0) openNote(next[0]);
        else { setActiveId(null); setTitle(""); setBody(""); }
      }
    } catch (e) {
      setErrorMsg(String(e));
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
              {transforms.length > 0 && (
                <div className="sp-transforms">
                  <span className="sp-transforms-label">
                    {hasSelection ? "Rewrite selection:" : "Select text to rewrite:"}
                  </span>
                  {transforms.map((t) => (
                    <button
                      key={t.id}
                      className="sp-transform-chip"
                      disabled={!hasSelection || runningTransformId != null}
                      onMouseDown={(e) => e.preventDefault()}
                      onClick={() => applyTransform(t)}
                      title={t.description || t.name}
                    >
                      {runningTransformId === t.id ? "Rewriting…" : t.name}
                    </button>
                  ))}
                  {transformError && <span className="sp-transform-err">{transformError}</span>}
                </div>
              )}
              <textarea
                ref={bodyRef}
                className="sp-body-input"
                placeholder="Start typing, or dictate with your hotkey…"
                value={body}
                onChange={(e) => onBodyChange(e.target.value)}
                onSelect={rememberSelection}
                onMouseUp={rememberSelection}
                onKeyUp={rememberSelection}
                onBlur={rememberSelection}
              />
              <button className="sp-copy" onClick={copyBody} disabled={!body}>
                <CopyIcon />
                {copyState === "copied" ? "Copied" : "Copy"}
              </button>
            </>
          )}
        </main>
      </div>

      <ConfirmDialog
        open={pendingDelete !== null}
        title="Delete this note?"
        message="This can't be undone."
        confirmLabel="Delete"
        danger
        onConfirm={confirmDelete}
        onCancel={() => setPendingDelete(null)}
      />
      <ConfirmDialog
        open={errorMsg !== null}
        title="Something went wrong"
        message={errorMsg}
        cancelLabel={null}
        onConfirm={() => setErrorMsg(null)}
      />
      <TooltipProvider />
    </div>
  );
}

function SpTitleBar() {
  const win = getCurrentWindow();
  // On macOS the OS owns the traffic-light controls — render an empty
  // drag region so the window stays draggable but we don't duplicate
  // the OS buttons. Cmd+W / red traffic-light close still fires
  // CloseRequested in Rust, which hides the window.
  if (IS_MAC) {
    return <div className="sp-titlebar" data-tauri-drag-region />;
  }
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
