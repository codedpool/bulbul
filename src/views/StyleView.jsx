import { useState } from "react";
import Combobox from "../components/Combobox.jsx";

const APP_CATEGORY_OPTIONS = [
  { code: "personal", label: "Personal" },
  { code: "work", label: "Work" },
  { code: "email", label: "Email" },
  { code: "other", label: "Other" },
];

const CATEGORIES = [
  {
    id: "personal",
    label: "Personal messages",
    field: "style_personal",
    apps: ["WhatsApp", "Telegram", "Signal", "Messenger"],
    blurb: "Style applies in personal messengers.",
  },
  {
    id: "work",
    label: "Work messages",
    field: "style_work",
    apps: ["Slack", "Teams", "Discord"],
    blurb: "Style applies in work messaging tools.",
  },
  {
    id: "email",
    label: "Email",
    field: "style_email",
    apps: ["Outlook", "Thunderbird", "Gmail"],
    blurb: "Style applies in email clients.",
  },
  {
    id: "other",
    label: "Other",
    field: "style_other",
    apps: ["Anywhere else"],
    blurb: "Default style for everything outside the categories above.",
  },
];

const STYLES = [
  {
    id: "formal",
    label: "Formal.",
    rule: "Caps + Punctuation",
    sample: "Hey, are you free for lunch tomorrow? Let's do 12 if that works for you.",
  },
  {
    id: "casual",
    label: "Casual",
    rule: "Caps + Less punctuation",
    sample: "Hey are you free for lunch tomorrow? Let's do 12 if that works for you",
  },
  {
    id: "very_casual",
    label: "very casual",
    rule: "No Caps + Less punctuation",
    sample: "hey are you free for lunch tomorrow? let's do 12 if that works for you",
  },
];

export default function StyleView({ config, updateConfig }) {
  const [activeCategory, setActiveCategory] = useState("personal");

  if (!config) return null;
  const cat = CATEGORIES.find((c) => c.id === activeCategory) || CATEGORIES[0];
  const selected = config[cat.field] || "casual";

  async function pick(styleId) {
    await updateConfig({ ...config, [cat.field]: styleId });
  }

  async function toggleEnabled(next) {
    await updateConfig({ ...config, style_enabled: next });
  }

  async function setOverrides(next) {
    await updateConfig({ ...config, style_app_overrides: next });
  }

  return (
    <div className="page style-page">
      <header className="page-header dictionary-header">
        <div>
          <h1>Style</h1>
          <p className="muted small">
            Apply a per-app tone to your cleaned transcripts. Bulbul detects the foreground app and quietly hints the cleanup model.
          </p>
        </div>
        <label className="style-master-toggle">
          <span className="muted small">{config.style_enabled ? "On" : "Off"}</span>
          <span className={`toggle ${config.style_enabled ? "on" : ""}`}>
            <input
              type="checkbox"
              checked={!!config.style_enabled}
              onChange={(e) => toggleEnabled(e.target.checked)}
            />
            <span className="toggle-thumb" />
          </span>
        </label>
      </header>

      <div className="style-tabs">
        {CATEGORIES.map((c) => (
          <button
            key={c.id}
            className={`tab ${activeCategory === c.id ? "active" : ""}`}
            onClick={() => setActiveCategory(c.id)}
          >
            {c.label}
          </button>
        ))}
      </div>

      <div className="style-banner">
        <div className="style-banner-text">
          <h2>This style applies in {cat.label.toLowerCase()}.</h2>
          <p className="muted small">
            {cat.blurb} Style is applied on top of your selected Cleanup mode.
          </p>
        </div>
        <div className="style-banner-apps">
          {cat.apps.map((a) => (
            <span key={a} className="style-app-chip">{a}</span>
          ))}
        </div>
      </div>

      <div className="style-cards">
        {STYLES.map((s) => (
          <StyleCard
            key={s.id}
            style={s}
            selected={selected === s.id}
            disabled={!config.style_enabled}
            onSelect={() => pick(s.id)}
          />
        ))}
      </div>

      <AppOverrides
        overrides={config.style_app_overrides || []}
        onChange={setOverrides}
        disabled={!config.style_enabled}
      />
    </div>
  );
}

function StyleCard({ style, selected, disabled, onSelect }) {
  return (
    <button
      className={`style-card ${selected ? "selected" : ""} ${disabled ? "disabled" : ""}`}
      onClick={onSelect}
      disabled={disabled}
    >
      <div className="style-card-top">
        <h3 className="style-card-title">{style.label}</h3>
        <p className="style-card-rule">{style.rule}</p>
      </div>
      <div className="style-sample">
        <p>{style.sample}</p>
      </div>
      {selected && <div className="style-card-check" aria-hidden>✓</div>}
    </button>
  );
}

function AppOverrides({ overrides, onChange, disabled }) {
  const [draftExe, setDraftExe] = useState("");
  const [draftCategory, setDraftCategory] = useState("work");

  function add() {
    const exe = draftExe.trim();
    if (!exe) return;
    const stem = exe.toLowerCase().replace(/\.exe$/, "");
    const dedup = overrides.filter(
      (o) => o.exe.toLowerCase().replace(/\.exe$/, "") !== stem,
    );
    onChange([...dedup, { exe, category: draftCategory }]);
    setDraftExe("");
  }

  function remove(idx) {
    onChange(overrides.filter((_, i) => i !== idx));
  }

  function updateRow(idx, patch) {
    onChange(overrides.map((o, i) => (i === idx ? { ...o, ...patch } : o)));
  }

  return (
    <section className={`style-overrides ${disabled ? "disabled" : ""}`}>
      <header className="style-overrides-header">
        <h3>Custom apps</h3>
        <p className="muted small">
          Map a specific executable to a category. Overrides Bulbul's built-in
          mappings — useful when an app you use (e.g. Cursor, Notion, Obsidian)
          doesn't fit the defaults.
        </p>
      </header>

      {overrides.length > 0 && (
        <ul className="style-overrides-list">
          {overrides.map((ov, i) => (
            <li key={i} className="style-override-row">
              <input
                type="text"
                value={ov.exe}
                onChange={(e) => updateRow(i, { exe: e.target.value })}
                placeholder="App.exe"
                disabled={disabled}
              />
              <Combobox
                value={ov.category}
                options={APP_CATEGORY_OPTIONS}
                onChange={(code) => updateRow(i, { category: code })}
                disabled={disabled}
                width={140}
                ariaLabel={`Category for ${ov.exe}`}
              />
              <button
                type="button"
                className="style-override-remove"
                onClick={() => remove(i)}
                disabled={disabled}
                aria-label={`Remove ${ov.exe}`}
                title="Remove"
              >
                ×
              </button>
            </li>
          ))}
        </ul>
      )}

      <div className="style-override-add">
        <input
          type="text"
          value={draftExe}
          onChange={(e) => setDraftExe(e.target.value)}
          onKeyDown={(e) => {
            if (e.key === "Enter") {
              e.preventDefault();
              add();
            }
          }}
          placeholder="e.g. Cursor.exe"
          disabled={disabled}
        />
        <Combobox
          value={draftCategory}
          options={APP_CATEGORY_OPTIONS}
          onChange={(code) => setDraftCategory(code)}
          disabled={disabled}
          width={140}
          ariaLabel="Category for new app"
        />
        <button
          type="button"
          className="primary"
          onClick={add}
          disabled={disabled || !draftExe.trim()}
        >
          Add
        </button>
      </div>
    </section>
  );
}
