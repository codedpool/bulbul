import { useEffect, useMemo, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import FeatureHero from "../components/FeatureHero.jsx";

const DICTIONARY_HERO_SAMPLES = [
  { trigger: "groq", expansion: "Groq" },
  { trigger: "github", expansion: "GitHub" },
  { trigger: "javascript", expansion: "JavaScript" },
];

export default function DictionaryView() {
  const [entries, setEntries] = useState([]);
  const [suggestions, setSuggestions] = useState([]);
  const [loading, setLoading] = useState(true);
  const [search, setSearch] = useState("");
  const [adding, setAdding] = useState(false);
  const [editingId, setEditingId] = useState(null);

  useEffect(() => { load(); }, []);

  async function load() {
    try {
      const [rows, sugg] = await Promise.all([
        invoke("list_dictionary"),
        invoke("correction_suggestions").catch(() => []),
      ]);
      setEntries(rows);
      setSuggestions(sugg);
    } catch (e) {
      console.error("dictionary load failed", e);
    } finally {
      setLoading(false);
    }
  }

  async function acceptSuggestion(s) {
    await invoke("add_dictionary_entry", {
      fromWord: s.from_word,
      toWord: s.to_word,
      caseSensitive: false,
    });
    await load();
  }

  async function dismissSuggestion(s) {
    // Optimistic: drop it from the list immediately, then persist.
    setSuggestions((prev) =>
      prev.filter((p) => !(p.from_word === s.from_word && p.to_word === s.to_word)),
    );
    await invoke("dismiss_correction_suggestion", {
      fromWord: s.from_word,
      toWord: s.to_word,
    });
  }

  async function addEntry(payload) {
    await invoke("add_dictionary_entry", {
      fromWord: payload.fromWord,
      toWord: payload.toWord,
      caseSensitive: payload.caseSensitive,
    });
    setAdding(false);
    await load();
  }

  async function saveEdit(id, payload) {
    await invoke("update_dictionary_entry", {
      id,
      fromWord: payload.fromWord,
      toWord: payload.toWord,
      caseSensitive: payload.caseSensitive,
    });
    setEditingId(null);
    await load();
  }

  async function remove(id) {
    await invoke("delete_dictionary_entry", { id });
    await load();
  }

  const filtered = useMemo(() => {
    const q = search.trim().toLowerCase();
    if (!q) return entries;
    return entries.filter(
      (e) =>
        e.from_word.toLowerCase().includes(q) ||
        e.to_word.toLowerCase().includes(q),
    );
  }, [entries, search]);

  return (
    <div className="page dictionary-page">
      <header className="page-header dictionary-header">
        <div>
          <h1>Dictionary</h1>
          <p className="muted small">
            Names, brands, jargon — Bulbul passes these to Whisper as a hint and substitutes them after transcription.
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
        dismissKey="bulbul.dictionary.hero.dismissed"
        title={<>Names, brands, jargon — <em>spelled right</em> every time.</>}
        samples={DICTIONARY_HERO_SAMPLES}
      />

      {suggestions.length > 0 && (
        <div className="dict-suggestions">
          <div className="dict-suggestions-head">
            <h3>Suggested from your edits</h3>
            <span className="muted small">
              Words you fixed by hand after dictating. Add the ones you want Bulbul to spell this way automatically.
            </span>
          </div>
          {suggestions.map((s) => (
            <div className="dict-suggestion-row" key={`${s.from_word}→${s.to_word}`}>
              <div className="dict-suggestion-main">
                <span className="dict-from">{s.from_word}</span>
                <span className="dict-arrow">→</span>
                <span className="dict-term">{s.to_word}</span>
                {s.count > 1 && (
                  <span className="dict-suggestion-count">fixed {s.count}×</span>
                )}
              </div>
              <div className="dict-suggestion-actions">
                <button className="primary small" onClick={() => acceptSuggestion(s)}>Add</button>
                <button className="small" onClick={() => dismissSuggestion(s)}>Dismiss</button>
              </div>
            </div>
          ))}
        </div>
      )}

      <div className="dict-toolbar">
        <div className="search-input">
          <SearchIcon />
          <input
            type="text"
            placeholder="Search entries…"
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
          <EntryForm
            initial={{ from_word: "", to_word: "", case_sensitive: false }}
            onSave={addEntry}
            onCancel={() => setAdding(false)}
          />
        )}

        {loading ? (
          <div className="empty-state"><p className="muted">Loading…</p></div>
        ) : filtered.length === 0 && !adding ? (
          <div className="empty-state">
            <p>{search ? "No matches." : "No dictionary entries yet. Click \"Add new\" to start."}</p>
          </div>
        ) : (
          filtered.map((e) => editingId === e.id ? (
            <EntryForm
              key={e.id}
              initial={e}
              onSave={(payload) => saveEdit(e.id, payload)}
              onCancel={() => setEditingId(null)}
            />
          ) : (
            <EntryRow
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

function EntryRow({ entry, onEdit, onDelete }) {
  const isSubstitution = entry.from_word.toLowerCase() !== entry.to_word.toLowerCase();
  return (
    <div className="dict-row">
      <div className="dict-row-main">
        {isSubstitution ? (
          <>
            <span className="dict-from">{entry.from_word}</span>
            <span className="dict-arrow">→</span>
            <span className="dict-term">{entry.to_word}</span>
          </>
        ) : (
          <span className="dict-term">{entry.to_word}</span>
        )}
        {entry.case_sensitive && <span className="dict-case-tag">Aa</span>}
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

function EntryForm({ initial, onSave, onCancel }) {
  const [term, setTerm] = useState(initial.to_word || "");
  const [showFrom, setShowFrom] = useState(
    !!initial.from_word && initial.from_word.toLowerCase() !== (initial.to_word || "").toLowerCase(),
  );
  const [fromWord, setFromWord] = useState(initial.from_word || "");
  const [caseSensitive, setCaseSensitive] = useState(!!initial.case_sensitive);
  const [error, setError] = useState("");

  async function submit() {
    const t = term.trim();
    if (!t) {
      setError("Term is required.");
      return;
    }
    const payload = {
      toWord: t,
      fromWord: showFrom && fromWord.trim() ? fromWord.trim() : t,
      caseSensitive,
    };
    try {
      await onSave(payload);
    } catch (e) {
      setError(String(e));
    }
  }

  function onKey(e) {
    if (e.key === "Enter") { e.preventDefault(); submit(); }
    if (e.key === "Escape") { e.preventDefault(); onCancel(); }
  }

  return (
    <div className="dict-form">
      <div className="dict-form-fields">
        {showFrom && (
          <>
            <input
              type="text"
              placeholder="when I say..."
              value={fromWord}
              onChange={(e) => setFromWord(e.target.value)}
              onKeyDown={onKey}
              spellCheck={false}
            />
            <span className="dict-arrow">→</span>
          </>
        )}
        <input
          type="text"
          placeholder={showFrom ? "...spell it like this" : "Term Bulbul should learn (e.g. \"Groq\", \"Romanch\")"}
          value={term}
          onChange={(e) => setTerm(e.target.value)}
          onKeyDown={onKey}
          autoFocus
          spellCheck={false}
        />
        <label className="case-toggle" title="Match case exactly when substituting">
          <input
            type="checkbox"
            checked={caseSensitive}
            onChange={(e) => setCaseSensitive(e.target.checked)}
          />
          <span>Aa</span>
        </label>
      </div>

      <div className="dict-form-actions">
        <button
          className={`text-btn ${showFrom ? "active" : ""}`}
          onClick={() => setShowFrom((v) => !v)}
          type="button"
        >
          {showFrom ? "− Remove auto-correct" : "+ Add an auto-correct"}
        </button>
        <div className="spacer" />
        <button onClick={onCancel}>Cancel</button>
        <button className="primary" onClick={submit} disabled={!term.trim()}>Save</button>
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
