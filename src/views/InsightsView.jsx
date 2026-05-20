import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";

export default function InsightsView() {
  const [tab, setTab] = useState("usage");

  return (
    <div className="page insights-page">
      <header className="page-header">
        <h1>Insights</h1>
        <div className="tabs">
          <button
            className={`tab ${tab === "usage" ? "active" : ""}`}
            onClick={() => setTab("usage")}
          >
            Your Usage
          </button>
          <button
            className={`tab ${tab === "voice" ? "active" : ""}`}
            onClick={() => setTab("voice")}
          >
            Your Voice
          </button>
        </div>
      </header>

      {tab === "usage" && <UsageTab />}
      {tab === "voice" && <VoiceTab />}
    </div>
  );
}

// ─────────────── Your Usage ───────────────

function UsageTab() {
  const [stats, setStats] = useState(null);
  const [loading, setLoading] = useState(true);

  useEffect(() => {
    let mounted = true;
    async function load() {
      try {
        const s = await invoke("get_insights_usage");
        if (mounted) setStats(s);
      } catch (e) {
        console.error("usage load failed", e);
      } finally {
        if (mounted) setLoading(false);
      }
    }
    load();
    const un = listen("bulbul-status", (e) => {
      if (e.payload.state === "done") load();
    });
    return () => { mounted = false; un.then((f) => f()); };
  }, []);

  if (loading) return <div className="empty-state"><p className="muted">Loading…</p></div>;
  if (!stats) return null;

  return (
    <div className="insights-grid">
      <div className="insight-card card-wpm">
        <div className="card-label">Words per minute</div>
        <div className="card-value-row">
          <div className="card-value-big">{Math.round(stats.wpm) || "—"}</div>
          <WpmGauge percentile={stats.wpm_percentile} />
        </div>
        <div className="card-subtitle">Top {formatPercentile(stats.wpm_percentile)}</div>
      </div>

      <div className="insight-card card-fixes">
        <div className="card-label">Fixes made by Bulbul</div>
        <div className="card-value-big">{formatNumber(stats.total_fixes)}</div>
        <div className="card-breakdown">
          <div className="breakdown-row">
            <span className="dot fix-ai" />
            <span>{formatNumber(stats.ai_fixes)} AI cleanup</span>
          </div>
          <div className="breakdown-row">
            <span className="dot fix-dict" />
            <span>{formatNumber(stats.dictionary_fixes)} dictionary</span>
          </div>
        </div>
      </div>

      <div className="insight-card card-words">
        <div className="card-header-row">
          <div>
            <div className="card-label">Total words dictated</div>
            <div className="card-value-big">{formatNumber(stats.total_words)}</div>
          </div>
          {stats.mom_change_pct != null && (
            <div className={`mom-badge ${stats.mom_change_pct >= 0 ? "up" : "down"}`}>
              {stats.mom_change_pct >= 0 ? "↑" : "↓"} {Math.abs(Math.round(stats.mom_change_pct))}% this month
            </div>
          )}
        </div>
        <div className="card-subtitle">
          {formatNumber(stats.words_this_month)} this month · {formatNumber(stats.words_last_month)} last month
        </div>
      </div>

      <div className="insight-panel panel-usage">
        <div className="panel-header">
          <h2>Desktop usage</h2>
          <span className="muted small">{stats.total_apps_used} apps used</span>
        </div>
        {stats.app_usage.length === 0 ? (
          <p className="muted small">No data yet — start dictating in different apps.</p>
        ) : (
          <div className="app-bars">
            {stats.app_usage.map((a) => (
              <AppBar
                key={a.category}
                category={a.category}
                count={a.count}
                percentage={a.percentage}
                isTop={a === stats.app_usage[0]}
              />
            ))}
          </div>
        )}
      </div>

      <div className="insight-panel panel-streak">
        <div className="panel-header">
          <h2>{stats.day_streak} day streak</h2>
          <span className="muted small">Longest streak · {stats.longest_streak} days</span>
        </div>
        <Heatmap days={stats.heatmap} />
        <div className="heatmap-legend">
          <span className="muted small">Less</span>
          <span className="legend-cell level-0" />
          <span className="legend-cell level-1" />
          <span className="legend-cell level-2" />
          <span className="legend-cell level-3" />
          <span className="legend-cell level-4" />
          <span className="muted small">More</span>
        </div>
      </div>
    </div>
  );
}

// ─────────────── Your Voice ───────────────

function VoiceTab() {
  const [voice, setVoice] = useState(null);
  const [loading, setLoading] = useState(true);
  const [refreshing, setRefreshing] = useState(false);
  const [refreshError, setRefreshError] = useState("");

  useEffect(() => {
    let mounted = true;
    async function load() {
      try {
        const v = await invoke("get_voice_stats");
        if (mounted) setVoice(v);
      } catch (e) {
        console.error("voice load failed", e);
      } finally {
        if (mounted) setLoading(false);
      }
    }
    load();
  }, []);

  async function refresh() {
    setRefreshing(true);
    setRefreshError("");
    try {
      const v = await invoke("refresh_voice_narrative");
      setVoice(v);
    } catch (e) {
      setRefreshError(String(e));
    } finally {
      setRefreshing(false);
    }
  }

  if (loading) return <div className="empty-state"><p className="muted">Loading…</p></div>;
  if (!voice) return null;

  const hasData = voice.total_words > 0;
  const canRefresh = voice.has_api_key && voice.total_words > 0;
  const readyForFirstGen = voice.last_generated_at == null;
  const wordsToReady = Math.max(0, voice.min_words_to_refresh - voice.total_words);

  return (
    <div className="voice-grid">
      <div className="voice-narrative-card">
        <div className="narrative-header">
          <div>
            <div className="card-label">Voice Profile</div>
            <h2 className="narrative-title">{deriveTitle(voice)}</h2>
          </div>
          <button
            className="refresh-btn"
            onClick={refresh}
            disabled={!canRefresh || refreshing || (readyForFirstGen && wordsToReady > 0)}
            title={
              !voice.has_api_key
                ? "Set your Groq API key in Settings first"
                : readyForFirstGen && wordsToReady > 0
                ? `Dictate ${wordsToReady} more words to unlock`
                : "Regenerate using Groq"
            }
          >
            <RefreshIcon spinning={refreshing} />
            {refreshing ? "Generating…" : voice.last_generated_at ? "Refresh" : "Generate"}
          </button>
        </div>

        {voice.voice_narrative ? (
          <p className="narrative-body">{voice.voice_narrative}</p>
        ) : (
          <p className="narrative-body muted">
            {readyForFirstGen && wordsToReady > 0
              ? `Voice profile uses your Groq key to generate a personalized summary. Dictate ${wordsToReady} more words to unlock.`
              : "No voice profile generated yet. Click Generate to create one from your recent dictations."}
          </p>
        )}

        {voice.last_generated_at && (
          <div className="narrative-meta">
            <span>Created {formatDate(voice.last_generated_at)}</span>
            <span className="dot-sep">·</span>
            <span>Next update suggested in {Math.max(0, voice.min_words_to_refresh - voice.words_since_last_gen)} more words</span>
          </div>
        )}

        {refreshError && <p className="err">{refreshError}</p>}
      </div>

      <div className="voice-stats-col">
        <VoiceStatTile
          big={voice.catchphrase ? `"${voice.catchphrase}"` : "—"}
          label="Catchphrase"
          italic
          big_size="md"
        />
        <VoiceStatTile
          big={voice.most_used_word ? `"${voice.most_used_word}"` : "—"}
          label="Most used word"
          italic
        />
        <VoiceStatTile
          big={voice.most_corrected_word ? `"${voice.most_corrected_word}"` : "—"}
          label="Most corrected word"
          italic
        />
      </div>

      <div className="voice-peak-card">
        <div className="peak-header">
          <CategoryIcon category={voice.peak_app_category || "Other"} large />
          <div>
            <h2 className="peak-title">
              {voice.peak_day_name && voice.peak_hour_label
                ? `${voice.peak_day_name} at ${voice.peak_hour_label}`
                : "Not enough data yet"}
            </h2>
            <div className="card-label">Your peak time &amp; place</div>
          </div>
        </div>
        <p className="peak-body">
          {voice.peak_narrative ||
            (voice.peak_app
              ? `Your most active time is ${voice.peak_day_name} around ${voice.peak_hour_label}, usually in ${stripExe(voice.peak_app)}.`
              : "Once you've dictated more, we'll surface when and where you're most active.")}
        </p>
      </div>

      {!hasData && (
        <div className="insights-empty voice-empty">
          <p>Dictate something to start building your voice profile.</p>
        </div>
      )}
    </div>
  );
}

function VoiceStatTile({ big, label, italic }) {
  return (
    <div className="voice-stat-tile">
      <div className={`voice-stat-big ${italic ? "italic" : ""}`}>{big}</div>
      <div className="voice-stat-label">{label}</div>
    </div>
  );
}

function deriveTitle(voice) {
  if (!voice.peak_app_category) return "Voice Profile";
  switch (voice.peak_app_category) {
    case "AI Prompts": return "Prompt Refiner";
    case "Code": return "Code Whisperer";
    case "Documents": return "The Writer";
    case "Emails": return "The Correspondent";
    case "Work messages": return "The Collaborator";
    case "Personal messages": return "The Conversationalist";
    case "Browsing": return "The Researcher";
    default: return "Voice Profile";
  }
}

// ─────────────── Shared ───────────────

function WpmGauge({ percentile }) {
  const fill = Math.max(2, 100 - percentile);
  return (
    <svg viewBox="0 0 100 60" className="wpm-gauge" aria-hidden>
      <path d="M 10 50 A 40 40 0 0 1 90 50" fill="none" stroke="#222" strokeWidth="8" strokeLinecap="round" />
      <path
        d="M 10 50 A 40 40 0 0 1 90 50"
        fill="none"
        stroke="#5ec8c0"
        strokeWidth="8"
        strokeLinecap="round"
        pathLength="100"
        strokeDasharray="100"
        strokeDashoffset={100 - fill}
      />
    </svg>
  );
}

function AppBar({ category, count, percentage, isTop }) {
  const pct = Math.round(percentage);
  return (
    <div className={`app-bar-row ${isTop ? "top" : ""}`}>
      <CategoryIcon category={category} />
      <div className="app-bar-name">{category}</div>
      <div className="app-bar-track">
        <div className="app-bar-fill" style={{ width: `${Math.max(3, percentage)}%` }} />
      </div>
      <div className="app-bar-stats">
        <span className="app-bar-pct">{pct}%</span>
        <span className="app-bar-count">{count}</span>
      </div>
    </div>
  );
}

function Heatmap({ days }) {
  const today = new Date();
  today.setHours(0, 0, 0, 0);
  const NUM_DAYS = 91; // 13 weeks
  const cells = [];
  const counts = Object.fromEntries(days.map((d) => [d.date, d.count]));
  for (let i = NUM_DAYS - 1; i >= 0; i--) {
    const d = new Date(today.getTime() - i * 86400000);
    const iso = localDateStr(d);
    cells.push({ date: iso, count: counts[iso] || 0, dow: d.getDay() });
  }
  const max = Math.max(1, ...cells.map((c) => c.count));

  // Arrange into 7-row columns by week.
  const columns = [];
  let buf = Array(7).fill(null);
  cells.forEach((c, i) => {
    if (i === 0) {
      buf = Array(7).fill(null);
      buf[c.dow] = c;
    } else {
      if (c.dow === 0 && buf.some(Boolean)) {
        columns.push(buf);
        buf = Array(7).fill(null);
      }
      buf[c.dow] = c;
    }
  });
  if (buf.some(Boolean)) columns.push(buf);

  return (
    <div className="heatmap">
      {columns.map((col, i) => (
        <div key={i} className="heatmap-col">
          {col.map((cell, j) => (
            <div
              key={j}
              className={`heatmap-cell ${cell ? `level-${levelFor(cell.count, max)}` : "empty"}`}
              title={cell ? `${cell.date} — ${cell.count} dictation${cell.count === 1 ? "" : "s"}` : ""}
            />
          ))}
        </div>
      ))}
    </div>
  );
}

// ─────────────── Icons ───────────────

function CategoryIcon({ category, large }) {
  const cls = `cat-icon ${large ? "large" : ""} cat-${slugify(category)}`;
  return (
    <span className={cls} aria-hidden>
      {renderCategoryIconSvg(category)}
    </span>
  );
}

function renderCategoryIconSvg(category) {
  const stroke = "currentColor";
  const props = { width: "100%", height: "100%", viewBox: "0 0 24 24", fill: "none", stroke, strokeWidth: 2, strokeLinecap: "round", strokeLinejoin: "round" };
  switch (category) {
    case "AI Prompts":
      return (
        <svg {...props}>
          <rect x="3" y="11" width="18" height="10" rx="2" />
          <circle cx="12" cy="5" r="2" />
          <path d="M12 7v4" />
          <line x1="8" y1="16" x2="8" y2="16" />
          <line x1="16" y1="16" x2="16" y2="16" />
        </svg>
      );
    case "Code":
      return (
        <svg {...props}>
          <polyline points="16 18 22 12 16 6" />
          <polyline points="8 6 2 12 8 18" />
        </svg>
      );
    case "Documents":
      return (
        <svg {...props}>
          <path d="M14 2H6a2 2 0 0 0-2 2v16a2 2 0 0 0 2 2h12a2 2 0 0 0 2-2V8z" />
          <polyline points="14 2 14 8 20 8" />
          <line x1="8" y1="13" x2="16" y2="13" />
          <line x1="8" y1="17" x2="13" y2="17" />
        </svg>
      );
    case "Emails":
      return (
        <svg {...props}>
          <rect x="2" y="4" width="20" height="16" rx="2" />
          <path d="m22 6-10 7L2 6" />
        </svg>
      );
    case "Work messages":
      return (
        <svg {...props}>
          <line x1="4" y1="9" x2="20" y2="9" />
          <line x1="4" y1="15" x2="20" y2="15" />
          <line x1="10" y1="3" x2="8" y2="21" />
          <line x1="16" y1="3" x2="14" y2="21" />
        </svg>
      );
    case "Personal messages":
      return (
        <svg {...props}>
          <path d="M21 15a2 2 0 0 1-2 2H7l-4 4V5a2 2 0 0 1 2-2h14a2 2 0 0 1 2 2z" />
        </svg>
      );
    case "Browsing":
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="10" />
          <line x1="2" y1="12" x2="22" y2="12" />
          <path d="M12 2a15.3 15.3 0 0 1 4 10 15.3 15.3 0 0 1-4 10 15.3 15.3 0 0 1-4-10 15.3 15.3 0 0 1 4-10z" />
        </svg>
      );
    case "System":
      return (
        <svg {...props}>
          <polyline points="4 17 10 11 4 5" />
          <line x1="12" y1="19" x2="20" y2="19" />
        </svg>
      );
    default:
      return (
        <svg {...props}>
          <circle cx="12" cy="12" r="3" />
        </svg>
      );
  }
}

function RefreshIcon({ spinning }) {
  return (
    <svg width="14" height="14" viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="2" strokeLinecap="round" strokeLinejoin="round" aria-hidden className={spinning ? "spin" : ""}>
      <path d="M21 12a9 9 0 1 1-3-6.7" />
      <polyline points="21 4 21 10 15 10" />
    </svg>
  );
}

// ─────────────── Helpers ───────────────

function levelFor(count, max) {
  if (count === 0) return 0;
  const ratio = count / max;
  if (ratio < 0.25) return 1;
  if (ratio < 0.5) return 2;
  if (ratio < 0.75) return 3;
  return 4;
}

function localDateStr(d) {
  const y = d.getFullYear();
  const m = String(d.getMonth() + 1).padStart(2, "0");
  const day = String(d.getDate()).padStart(2, "0");
  return `${y}-${m}-${day}`;
}

function formatNumber(n) {
  if (n == null) return "—";
  if (n >= 10000) return `${Math.round(n / 100) / 10}K`;
  if (n >= 1000) return n.toLocaleString();
  return n.toString();
}

function formatPercentile(p) {
  if (p < 1) return `${p.toFixed(1)}%`;
  return `${Math.round(p)}%`;
}

function formatDate(unix) {
  const d = new Date(unix * 1000);
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric", year: "numeric" });
}

function stripExe(name) {
  return name.replace(/\.exe$/i, "");
}

function slugify(s) {
  return s.toLowerCase().replace(/\s+/g, "-");
}
