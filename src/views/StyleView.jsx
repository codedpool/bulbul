import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";

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
