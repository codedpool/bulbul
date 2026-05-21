import { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import FeatureHero from "../components/FeatureHero.jsx";

const PAGE_SIZE = 50;

export default function HomeView() {
  const [stats, setStats] = useState(null);
  const [recent, setRecent] = useState([]);
  const [hasMore, setHasMore] = useState(true);
  const [loadingMore, setLoadingMore] = useState(false);

  useEffect(() => {
    let mounted = true;
    async function load() {
      try {
        const [s, r] = await Promise.all([
          invoke("get_home_stats"),
          invoke("get_recent_dictations", { limit: PAGE_SIZE, offset: 0 }),
        ]);
        if (mounted) {
          setStats(s);
          setRecent(r);
          setHasMore(r.length === PAGE_SIZE);
        }
      } catch (e) {
        console.error("home load failed", e);
      }
    }
    load();

    const un = listen("bulbul-status", (e) => {
      if (e.payload.state === "done") load();
    });
    return () => {
      mounted = false;
      un.then((f) => f());
    };
  }, []);

  async function loadOlder() {
    if (loadingMore || !hasMore) return;
    setLoadingMore(true);
    try {
      const next = await invoke("get_recent_dictations", {
        limit: PAGE_SIZE,
        offset: recent.length,
      });
      setRecent((prev) => [...prev, ...next]);
      setHasMore(next.length === PAGE_SIZE);
    } catch (e) {
      console.error("load older failed", e);
    } finally {
      setLoadingMore(false);
    }
  }

  const grouped = groupByDay(recent);

  return (
    <div className="page home">
      <header className="page-header">
        <h1>Welcome back</h1>
        <p className="muted small">Your dictation activity, all local. No data leaves your machine except to Groq.</p>
      </header>

      <FeatureHero
        dismissKey="bulbul.home.hero.dismissed"
        title={<>Speak. Edit. <em>Move on.</em></>}
        blurb="Hold your hotkey anywhere on Windows, talk, release. Bulbul transcribes, cleans up, and pastes the result right at your cursor."
      />

      <section className="stat-cards">
        <StatCard
          label="Total words"
          value={stats ? formatNumber(stats.total_words) : "—"}
          subtitle={stats ? `${formatNumber(stats.total_dictations)} dictations` : ""}
        />
        <StatCard
          label="Words per minute"
          value={stats ? Math.round(stats.wpm_7d).toString() : "—"}
          subtitle="7-day average"
        />
        <StatCard
          label="Day streak"
          value={stats ? `${stats.day_streak}` : "—"}
          subtitle={stats?.day_streak === 1 ? "day" : "days in a row"}
        />
        <StatCard
          label="Fixes by Bulbul"
          value={stats ? formatNumber(stats.total_fixes) : "—"}
          subtitle="Words cleaned up"
        />
      </section>

      <section className="timeline-section">
        <h3>Recent activity</h3>
        {recent.length === 0 ? (
          <div className="empty-state">
            <p>No dictations yet. Hold your hotkey anywhere in Windows and start talking.</p>
          </div>
        ) : (
          <div className="timeline">
            {grouped.map(({ day, items }) => (
              <div key={day} className="day-group">
                <div className="day-label">{day}</div>
                <div className="day-card">
                  {items.map((d) => (
                    <div key={d.id} className="dictation-row">
                      <div className="dictation-time">{formatTime(d.ts)}</div>
                      <div className="dictation-body">
                        <div className="dictation-text">{d.cleaned_text}</div>
                        <div className="dictation-meta">
                          {d.foreground_app && <span className="badge">{stripExe(d.foreground_app)}</span>}
                          <span className="badge muted-badge">{d.mode}</span>
                          <span className="badge muted-badge">{d.word_count}w</span>
                        </div>
                      </div>
                    </div>
                  ))}
                </div>
              </div>
            ))}
            {hasMore && (
              <button
                className="load-older"
                onClick={loadOlder}
                disabled={loadingMore}
              >
                {loadingMore ? "Loading…" : "Load older activity"}
              </button>
            )}
          </div>
        )}
      </section>
    </div>
  );
}

function StatCard({ label, value, subtitle }) {
  return (
    <div className="stat-card">
      <div className="stat-value">{value}</div>
      <div className="stat-label">{label}</div>
      {subtitle && <div className="stat-subtitle">{subtitle}</div>}
    </div>
  );
}

function formatNumber(n) {
  if (n == null) return "—";
  if (n >= 1000) return `${(n / 1000).toFixed(1)}K`;
  return n.toString();
}

function formatTime(unix) {
  const d = new Date(unix * 1000);
  return d.toLocaleTimeString(undefined, { hour: "numeric", minute: "2-digit" });
}

function stripExe(name) {
  return name.replace(/\.exe$/i, "");
}

function groupByDay(rows) {
  const out = [];
  let currentDay = null;
  for (const r of rows) {
    const day = dayLabel(r.ts);
    if (day !== currentDay) {
      out.push({ day, items: [] });
      currentDay = day;
    }
    out[out.length - 1].items.push(r);
  }
  return out;
}

function dayLabel(unix) {
  const d = new Date(unix * 1000);
  const now = new Date();
  const oneDay = 86_400_000;
  const startOfToday = new Date(now.getFullYear(), now.getMonth(), now.getDate()).getTime();
  const startOfRow = new Date(d.getFullYear(), d.getMonth(), d.getDate()).getTime();
  const diff = startOfToday - startOfRow;
  if (diff === 0) return "Today";
  if (diff === oneDay) return "Yesterday";
  if (diff < 7 * oneDay) return d.toLocaleDateString(undefined, { weekday: "long" });
  return d.toLocaleDateString(undefined, { month: "short", day: "numeric" });
}
