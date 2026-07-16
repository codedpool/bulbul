import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FeatureHero from "../components/FeatureHero.jsx";
import ConfirmDialog from "../components/ConfirmDialog.jsx";
import { IS_ANDROID } from "../platform.js";

const AUTOSAVE_DELAY_MS = 600;

export default function ScratchpadView() {
  const [notes, setNotes] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [activeId, setActiveId] = useState(null);
  const [title, setTitle] = useState("");
  const [body, setBody] = useState("");
  const [saveState, setSaveState] = useState("idle"); // idle | dirty | saving | saved
  const [transforms, setTransforms] = useState([]);
  const [runningTransformId, setRunningTransformId] = useState(null);
  const [transformError, setTransformError] = useState("");
  const [hasSelection, setHasSelection] = useState(false);
  const saveTimer = useRef(null);
  const dirtyRef = useRef(false);
  const bodyRef = useRef(null);
  const selRef = useRef({ start: 0, end: 0 });
  const applyTransformRef = useRef(null);
  const transformsRef = useRef([]);

  useEffect(() => {
    load();
    invoke("list_transforms").then(setTransforms).catch(() => {});
    const un = listen("notes-changed", () => loadKeepSelection());
    return () => { un.then((f) => f()); };
  }, []);

  // Dictation into the INLINE scratchpad view (dashboard sidebar →
  // Scratchpad). The orchestrator emits `bulbul-focused-insert` to the
  // main window when Bulbul is foreground on Mac but the standalone
  // scratchpad isn't the target. We only consume when this textarea is
  // the current document focus — a stray emit from Home/Insights is a
  // silent no-op that way, and the standalone window's own listener
  // (which fires on `scratchpad-append`, a different event) still owns
  // its case.
  useEffect(() => {
    const un = listen("bulbul-focused-insert", (event) => {
      const incoming = String(event.payload || "");
      if (!incoming) return;
      const el = bodyRef.current;
      if (!el || document.activeElement !== el) return;
      const start = el.selectionStart;
      const end = el.selectionEnd;
      setBody((prev) => {
        dirtyRef.current = true;
        return prev.slice(0, start) + incoming + prev.slice(end);
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
  }, []);

  function rememberSelection() {
    const el = bodyRef.current;
    if (!el) return;
    selRef.current = { start: el.selectionStart, end: el.selectionEnd };
    setHasSelection(el.selectionEnd > el.selectionStart);
  }

  // TODO(v1.1.1): transforms here only fire when the user CLICKS a chip.
  // Pressing the transform hotkey (e.g. Cmd+1 on macOS) inside this
  // in-dashboard scratchpad does nothing — the global-shortcut path
  // captures the OS selection and never sees this webview textarea. When
  // Bulbul's own window is focused, a transform hotkey should call this
  // applyTransform on the current textarea selection instead. (See the
  // format_combo TODO in hotkey/mod.rs for the companion label/edit work.)
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
      // Put the caret just after the rewritten span on the next paint.
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

  // Keep refs to the latest runner + transform list so the global-hotkey
  // listener (subscribed once) never calls a stale closure.
  applyTransformRef.current = applyTransform;
  transformsRef.current = transforms;

  // The global transform hotkey (⌘/Alt+1..9) is routed here by the backend
  // when Bulbul's own window is focused (it emits "run-transform-in-app"),
  // so it can act on THIS webview textarea's selection — the OS-selection
  // pipeline can't reach a webview, which is why the hotkey did nothing in
  // the in-dashboard scratchpad on macOS. Same path as clicking the chip;
  // no-ops when nothing is selected here.
  useEffect(() => {
    const un = listen("run-transform-in-app", (event) => {
      const el = bodyRef.current;
      if (!el || document.activeElement !== el) return;
      selRef.current = { start: el.selectionStart, end: el.selectionEnd };
      if (el.selectionEnd <= el.selectionStart) return;
      const t = transformsRef.current.find((x) => x.id === Number(event.payload));
      if (t) applyTransformRef.current?.(t);
    });
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
      setErrorMsg(String(e));
    }
  }

  // Note pending confirmation for deletion. Holds the id (or null when no
  // delete is in flight) and drives the ConfirmDialog visibility.
  const [pendingDelete, setPendingDelete] = useState(null);
  // Error string for the themed alert (save/delete failures). null = closed.
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

  async function backToList() {
    if (dirtyRef.current) await flushSave();
    setActiveId(null);
    setTitle("");
    setBody("");
    setTransformError("");
  }

  // ─────────── Mobile: master (note list) / detail (full editor) ───────────
  if (IS_ANDROID) {
    const inEditor = activeId != null;
    return (
      <div className="page scratchpad-page m-scratch">
        {!inEditor ? (
          <div className="m-scratch-list-view">
            <header className="m-scratch-list-head">
              <h1>Scratchpad</h1>
              <button className="primary" onClick={startNewNote}>
                <PlusIcon /> New note
              </button>
            </header>
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
            <div className="m-note-stack">
              {loading ? (
                <div className="muted small list-empty">Loading…</div>
              ) : filtered.length === 0 ? (
                <div className="empty-state">
                  <p className="muted">{search ? "No matches." : "No notes yet. Tap \"New note\" to start."}</p>
                </div>
              ) : (
                filtered.map((n) => (
                  <button className="m-note-card" key={n.id} onClick={() => openNote(n)}>
                    <span className="m-note-card-main">
                      <span className="m-note-card-title">{n.title || "Untitled"}</span>
                      <span className="m-note-card-preview">
                        {(n.body || "").trim().split("\n")[0] || "(no content)"}
                      </span>
                    </span>
                    <span className="m-note-card-meta">
                      <span className="m-note-card-time">{relativeTime(n.updated_at)}</span>
                      <span
                        className="m-note-card-del"
                        role="button"
                        aria-label="Delete"
                        onClick={(e) => { e.stopPropagation(); removeNote(n.id); }}
                      >
                        <TrashIcon />
                      </span>
                    </span>
                  </button>
                ))
              )}
            </div>
          </div>
        ) : (
          <div className="m-scratch-editor-view">
            <div className="m-scratch-editor-head">
              <button className="m-icon-btn" onClick={backToList} aria-label="Back to notes">
                <BackArrowIcon />
              </button>
              <input
                type="text"
                className="scratch-title m-scratch-title"
                placeholder="Untitled"
                value={title}
                onChange={(e) => onTitleChange(e.target.value)}
                spellCheck={false}
              />
              <SaveBadge state={saveState} />
            </div>
            {transforms.length > 0 && (
              <div className="scratch-transforms">
                <span className="scratch-transforms-label">
                  {hasSelection ? "Rewrite selection:" : "Select text to rewrite:"}
                </span>
                {transforms.map((t) => (
                  <button
                    key={t.id}
                    className="scratch-transform-chip"
                    disabled={!hasSelection || runningTransformId != null}
                    onMouseDown={(e) => e.preventDefault()}
                    onClick={() => applyTransform(t)}
                    title={t.description || t.name}
                  >
                    {runningTransformId === t.id ? "Rewriting…" : t.name}
                  </button>
                ))}
              </div>
            )}
            {transformError && <div className="scratch-transform-err m-scratch-err">{transformError}</div>}
            <textarea
              ref={bodyRef}
              className="scratch-body m-scratch-body"
              placeholder="Start typing, or tap the floating bubble to dictate…"
              value={body}
              onChange={(e) => onBodyChange(e.target.value)}
              onSelect={rememberSelection}
              onMouseUp={rememberSelection}
              onKeyUp={rememberSelection}
              onBlur={rememberSelection}
            />
          </div>
        )}

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
      </div>
    );
  }

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

      <FeatureHero
        dismissKey="bulbul.scratchpad.hero.dismissed"
        title={<>Quick thoughts you <em>don't want to lose.</em></>}
        blurb="Dictate or type freely. Notes auto-save as you go — no save buttons, no folders, just a place for the things that would otherwise live in your head."
      />

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
              {transforms.length > 0 && (
                <div className="scratch-transforms">
                  <span className="scratch-transforms-label">
                    {hasSelection ? "Rewrite selection:" : "Select text to rewrite:"}
                  </span>
                  {transforms.map((t) => (
                    <button
                      key={t.id}
                      className="scratch-transform-chip"
                      disabled={!hasSelection || runningTransformId != null}
                      onMouseDown={(e) => e.preventDefault()} /* keep textarea selection */
                      onClick={() => applyTransform(t)}
                      title={t.description || t.name}
                    >
                      {runningTransformId === t.id ? "Rewriting…" : t.name}
                    </button>
                  ))}
                  {transformError && <span className="scratch-transform-err">{transformError}</span>}
                </div>
              )}
              <textarea
                ref={bodyRef}
                className="scratch-body"
                placeholder="Start typing, or dictate with your hotkey…"
                value={body}
                onChange={(e) => onBodyChange(e.target.value)}
                onSelect={rememberSelection}
                onMouseUp={rememberSelection}
                onKeyUp={rememberSelection}
                onBlur={rememberSelection}
              />
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

function BackArrowIcon() {
  return (
    <svg width="24" height="24" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden>
      <line x1="19" y1="12" x2="5" y2="12" />
      <polyline points="12 19 5 12 12 5" />
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
