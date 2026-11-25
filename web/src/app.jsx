import { createEffect, createMemo, createSignal, For, onCleanup, onMount, Show } from "solid-js";
import {
  addPushSubscription,
  getDay,
  getKeeper,
  getModel,
  getPushKey,
  getQuota,
  getSettings,
  getState,
  getUsage,
  pair,
  patchSettings,
  pingKeeper,
  refresh,
  removePushSubscription,
} from "./api";
import {
  formatCost,
  formatNumber,
  formatPercent,
  isChinese,
  locale,
  t,
} from "./i18n";

const agents = [
  { id: "claude", label: "Claude", color: "#d97757" },
  { id: "codex", label: "Codex", color: "#10a37f" },
  { id: "copilot", label: "Copilot", color: "#8b5cf6" },
  { id: "gemini", label: "Gemini", color: "#4285f4" },
  { id: "opencode", label: "OpenCode", color: "#f59e0b" },
  { id: "pi", label: "Pi", color: "#ec4899" },
  { id: "agy", label: "Antigravity", color: "#22c55e" },
];
const agentLabel = (id) => agents.find((agent) => agent.id === id)?.label || id;

function today() {
  const now = new Date();
  return `${now.getFullYear()}-${String(now.getMonth() + 1).padStart(2, "0")}-${String(now.getDate()).padStart(2, "0")}`;
}

function parseDate(value) {
  const [year, month, day] = value.split("-").map(Number);
  return new Date(year, month - 1, day);
}

function dateKey(date) {
  return `${date.getFullYear()}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function addDays(value, count) {
  const date = typeof value === "string" ? parseDate(value) : new Date(value);
  date.setDate(date.getDate() + count);
  return date;
}

function formatDateTime(value) {
  if (!value) return "—";
  return new Date(value).toLocaleString(locale(), { month: "short", day: "numeric", hour: "2-digit", minute: "2-digit" });
}

function formatReset(value) {
  if (!value) return "";
  const date = new Date(value);
  const diff = date.getTime() - Date.now();
  if (diff <= 0) return date.toLocaleString(locale(), { month: "short", day: "numeric" });
  const hours = Math.floor(diff / 3600000);
  const minutes = Math.floor((diff % 3600000) / 60000);
  return `${hours}h ${minutes}m`;
}

function cacheHit(totals) {
  if (!totals || !totals.tokens) return 0;
  return (totals.cacheRead / totals.tokens) * 100;
}

function percentChange(current, previous) {
  if (!previous) return current ? 100 : 0;
  return ((current - previous) / previous) * 100;
}

function App() {
  const [paired, setPaired] = createSignal(null);
  const [state, setState] = createSignal(null);
  const [page, setPage] = createSignal("usage");
  const [toast, setToast] = createSignal("");
  const [usageNonce, setUsageNonce] = createSignal(0);
  const [quotaNonce, setQuotaNonce] = createSignal(0);
  const [keeperNonce, setKeeperNonce] = createSignal(0);
  const [lastRefresh, setLastRefresh] = createSignal(null);
  const [busy, setBusy] = createSignal(false);
  const [online, setOnline] = createSignal(navigator.onLine);
  const [bootError, setBootError] = createSignal("");

  createEffect(() => {
    if (toast()) {
      const timer = setTimeout(() => setToast(""), 3200);
      onCleanup(() => clearTimeout(timer));
    }
  });

  async function loadState() {
    const value = await getState();
    setState(value);
    setPaired(true);
    return value;
  }

  async function initialize() {
    setBootError("");
    try {
      const hash = window.location.hash;
      if (hash.startsWith("#token=")) {
        const token = decodeURIComponent(hash.slice("#token=".length));
        history.replaceState(null, "", window.location.pathname + window.location.search);
        try {
          await pair(token);
        } catch (error) {
          setPaired(false);
          return;
        }
      }
      await loadState();
    } catch (error) {
      setPaired(false);
      if (error.status !== 401) {
        setBootError(error.message || t("offline"));
      }
    }
  }

  onMount(() => {
    initialize();
    const updateOnline = () => setOnline(navigator.onLine);
    const unauthorized = () => setPaired(false);
    window.addEventListener("online", updateOnline);
    window.addEventListener("offline", updateOnline);
    window.addEventListener("tokenpulse-unauthorized", unauthorized);
    let stopped = false;
    let socket;
    let attempt = 0;
    const connect = () => {
      if (stopped || socket) return;
      const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
      socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
      socket.onopen = () => {
        attempt = 0;
        setUsageNonce((value) => value + 1);
        setQuotaNonce((value) => value + 1);
        setKeeperNonce((value) => value + 1);
      };
      socket.onmessage = (event) => {
        let payload;
        try {
          payload = JSON.parse(event.data);
        } catch {
          return;
        }
        if (payload.type === "usage_updated") {
          setUsageNonce((value) => value + 1);
          setLastRefresh(new Date());
        } else if (payload.type === "quota_updated") {
          setQuotaNonce((value) => value + 1);
          setLastRefresh(new Date());
        } else if (payload.type === "keeper_updated") {
          setKeeperNonce((value) => value + 1);
        } else if (payload.type === "settings_updated") {
          loadState().catch(() => {});
          setQuotaNonce((value) => value + 1);
        } else if (payload.type === "quota_recovered") {
          setToast(`${agentLabel(payload.data.provider)} ${payload.data.window} · ${Math.round(payload.data.remaining)}%`);
        }
      };
      socket.onclose = () => {
        socket = undefined;
        if (stopped) return;
        attempt += 1;
        setTimeout(connect, Math.min(30000, 1000 * 2 ** attempt));
      };
    };
    createEffect(() => {
      if (paired() === true) connect();
    });
    onCleanup(() => {
      stopped = true;
      socket?.close();
      window.removeEventListener("online", updateOnline);
      window.removeEventListener("offline", updateOnline);
      window.removeEventListener("tokenpulse-unauthorized", unauthorized);
    });
  });

  async function doRefresh() {
    setBusy(true);
    try {
      if (page() === "usage") {
        await refresh(true, false);
        setUsageNonce((value) => value + 1);
      } else if (page() === "quota") {
        await refresh(false, true);
        setQuotaNonce((value) => value + 1);
      } else {
        await refresh(true, true);
        setUsageNonce((value) => value + 1);
        setQuotaNonce((value) => value + 1);
        setKeeperNonce((value) => value + 1);
      }
      setLastRefresh(new Date());
    } catch (error) {
      setToast(error.message);
    } finally {
      setBusy(false);
    }
  }

  function applySettings(updates) {
    setState((current) => {
      if (!current) return current;
      const settings = { ...(current.settings || {}) };
      for (const [key, value] of Object.entries(updates)) settings[key] = value;
      return { ...current, settings };
    });
  }

  return (
    <div class="app">
      <Show when={paired() === null}>
        <div class="center">
          <div class="spinner" />
        </div>
      </Show>
      <Show when={paired() === false}>
        <PairScreen
          error={bootError()}
          onRetry={initialize}
          onPaired={async () => { await loadState(); }}
          toast={setToast}
        />
      </Show>
      <Show when={paired() === true}>
        <header class="topbar">
          <div class="brand">
            <span class="pulse" />
            <div>
              <strong>TokenPulse</strong>
              <small>
                {state()?.version}
                <Show when={!online()}> · {t("offline")}</Show>
              </small>
            </div>
          </div>
          <div class="top-actions">
            <small class="last-refresh">{lastRefresh() ? formatDateTime(lastRefresh()) : ""}</small>
            <button class="icon-button" classList={{ spin: busy() }} onClick={doRefresh} aria-label={t("refresh")}>
              ↻
            </button>
          </div>
        </header>

        <main class="main">
          <Show when={page() === "usage"}>
            <UsagePage state={state()} nonce={usageNonce()} toast={setToast} />
          </Show>
          <Show when={page() === "quota"}>
            <QuotaPage state={state()} nonce={quotaNonce()} toast={setToast} />
          </Show>
          <Show when={page() === "keeper"}>
            <KeeperPage nonce={keeperNonce()} state={state()} toast={setToast} onSettings={applySettings} />
          </Show>
          <Show when={page() === "settings"}>
            <SettingsPage state={state()} toast={setToast} onSettings={applySettings} />
          </Show>
        </main>

        <nav class="bottom-nav">
          <For each={[["usage", "◔", t("usage")], ["quota", "◍", t("quota")], ["keeper", "◉", t("keeper")], ["settings", "☰", t("settings")]]}>
            {([id, icon, label]) => (
              <button classList={{ active: page() === id }} onClick={() => setPage(id)}>
                <span>{icon}</span>
                <em>{label}</em>
              </button>
            )}
          </For>
        </nav>
        <Show when={toast()}>
          <div class="toast">{toast()}</div>
        </Show>
      </Show>
    </div>
  );
}

function PairScreen(props) {
  const [token, setToken] = createSignal("");
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal(false);
  async function submit(event) {
    event.preventDefault();
    setBusy(true);
    setError("");
    try {
      await pair(token().trim());
      await props.onPaired();
    } catch (err) {
      setError(err.message || t("pairFailed"));
    } finally {
      setBusy(false);
    }
  }
  return (
    <div class="pair-screen">
      <div class="pair-card">
        <div class="brand large">
          <span class="pulse" />
          <strong>TokenPulse</strong>
        </div>
        <h1>{t("pairTitle")}</h1>
        <p>{t("pairBody")}</p>
        <Show when={props.error}>
          <div class="error-box">
            <span>{t("offline")}: {props.error}</span>
            <button onClick={props.onRetry}>{t("retry")}</button>
          </div>
        </Show>
        <form onSubmit={submit}>
          <label>
            {t("pairToken")}
            <input value={token()} onInput={(event) => setToken(event.currentTarget.value)} autocomplete="off" />
          </label>
          <Show when={error()}><p class="error-text">{error()}</p></Show>
          <button class="primary" type="submit" disabled={busy() || !token().trim()}>
            {busy() ? t("refreshing") : t("pairAction")}
          </button>
        </form>
      </div>
    </div>
  );
}

function UsagePage(props) {
  const enabledAgents = () => {
    const listed = props.state?.agents || agents.map((agent) => agent.id);
    return agents.filter((agent) => listed.includes(agent.id));
  };
  const [preset, setPreset] = createSignal("7d");
  const [from, setFrom] = createSignal(today());
  const [to, setTo] = createSignal(today());
  const [selected, setSelected] = createSignal(new Set(enabledAgents().map((agent) => agent.id)));
  const [metric, setMetric] = createSignal("tokens");
  const [report, setReport] = createSignal(null);
  const [error, setError] = createSignal("");
  const [loading, setLoading] = createSignal(false);
  const [dayLimit, setDayLimit] = createSignal(10);
  const [modelLimit, setModelLimit] = createSignal(10);
  const [sheet, setSheet] = createSignal(null);

  const agentParam = () => [...selected()].join(",");

  async function load() {
    if (!selected().size) return;
    setLoading(true);
    setError("");
    try {
      const params = { preset: preset(), agents: agentParam() };
      if (preset() === "custom") {
        params.from = from();
        params.to = to();
      }
      setReport(await getUsage(params));
      setDayLimit(10);
      setModelLimit(10);
    } catch (err) {
      setError(err.message || t("error"));
    } finally {
      setLoading(false);
    }
  }

  createEffect(() => {
    props.nonce;
    preset();
    selected();
    load();
  });

  function choosePreset(value) {
    setPreset(value);
  }

  function toggleAgent(id) {
    const next = new Set(selected());
    if (next.has(id)) {
      if (next.size === 1) return;
      next.delete(id);
    } else {
      next.add(id);
    }
    setSelected(next);
  }

  const totals = () => report()?.totals;
  const comparison = () => (report()?.comparison ? report().comparison.totals : null);
  const showComparison = () => report() && report().range.from !== report().range.to;

  const change = (key) => {
    if (!comparison()) return null;
    return percentChange(totals()?.[key] || 0, comparison()?.[key] || 0);
  };

  return (
    <section class="page">
      <div class="preset-row">
        <For each={[["today", t("today")], ["7d", t("last7")], ["30d", t("last30")], ["year", t("year")], ["all", t("all")], ["custom", t("custom")]]}>
          {([id, label]) => (
            <button classList={{ chip: true, active: preset() === id }} onClick={() => choosePreset(id)}>{label}</button>
          )}
        </For>
      </div>
      <Show when={preset() === "custom"}>
        <div class="custom-range">
          <label>{t("from")}<input type="date" value={from()} max={today()} onInput={(event) => setFrom(event.currentTarget.value)} /></label>
          <label>{t("to")}<input type="date" value={to()} max={today()} onInput={(event) => setTo(event.currentTarget.value)} /></label>
          <button class="primary small" onClick={load}>{t("refresh")}</button>
        </div>
      </Show>

      <div class="agent-row">
        <For each={enabledAgents()}>
          {(agent) => (
            <button
              classList={{ agent: true, active: selected().has(agent.id) }}
              style={{ "--agent": agent.color }}
              onClick={() => toggleAgent(agent.id)}
            >
              <i />{agent.label}
            </button>
          )}
        </For>
      </div>

      <Show when={error()}>
        <div class="error-box">
          <span>{t("error")}: {error()}</span>
          <button onClick={load}>{t("retry")}</button>
        </div>
      </Show>
      <Show when={loading() && !report()}>
        <div class="center"><div class="spinner" /></div>
      </Show>
      <Show when={report()}>
        <div class="metric-switch">
          <button classList={{ active: metric() === "tokens" }} onClick={() => setMetric("tokens")}>{t("tokens")}</button>
          <button classList={{ active: metric() === "cost" }} onClick={() => setMetric("cost")}>{t("cost")}</button>
        </div>

        <div class="kpi-grid">
          <Kpi label={t("totalTokens")} value={formatNumber(totals()?.tokens)} change={showComparison() ? change("tokens") : null} />
          <Kpi label={t("totalCost")} value={formatCost(totals()?.cost)} change={showComparison() ? change("cost") : null} />
          <Kpi label={t("messages")} value={formatNumber(totals()?.messages)} change={showComparison() ? change("messages") : null} />
          <Kpi label={t("cacheHit")} value={formatPercent(cacheHit(totals()), 2)} />
        </div>
        <div class="secondary-totals">
          <span>{t("sessions")}: {formatNumber(totals()?.sessions)}</span>
          <span>{t("activeDays")}: {formatNumber(report()?.days?.length)}</span>
          <Show when={report()?.comparison}>
            <span>{t("comparison")}: {report().comparison.from} → {report().comparison.to}</span>
          </Show>
        </div>
        <p class="note">{t("estimate")}</p>
        <Show when={report()?.models?.some((model) => model.unpriced)}>
          <p class="note warning">{t("unpriced")}</p>
        </Show>

        <Show when={report()?.days?.length} fallback={<div class="empty">{t("noUsage")}</div>}>
          <Heatmap report={report()} metric={metric()} onDay={openDay} />
        </Show>

        <ListSection
          title={t("days")}
          rows={report()?.days || []}
          limit={dayLimit()}
          onMore={() => setDayLimit((value) => value + 20)}
          render={(day) => (
            <button class="list-row" onClick={() => openDay(day.date)}>
              <div>
                <strong>{day.date}</strong>
                <small>{formatNumber(day.messages)} {t("messages")} · {formatNumber(day.sessions)} {t("sessions")}</small>
              </div>
              <div class="right">
                <strong>{metric() === "cost" ? formatCost(day.cost) : formatNumber(day.tokens)}</strong>
                <small>{metric() === "cost" ? formatNumber(day.tokens) : formatCost(day.cost)}</small>
              </div>
            </button>
          )}
        />

        <ListSection
          title={t("models")}
          rows={report()?.models || []}
          limit={modelLimit()}
          onMore={() => setModelLimit((value) => value + 20)}
          render={(model) => (
            <button class="list-row" onClick={() => openModel(model.model)}>
              <div>
                <strong>{model.model}</strong>
                <small>{model.company} · {(model.agents || []).map(agentLabel).join(", ")}</small>
              </div>
              <div class="right">
                <strong>{metric() === "cost" ? formatCost(model.cost) : formatNumber(model.tokens)}</strong>
                <small>{metric() === "cost" ? formatNumber(model.tokens) : formatCost(model.cost)}</small>
              </div>
            </button>
          )}
        />
      </Show>

      <Show when={sheet()}>
        <Sheet onClose={() => setSheet(null)}>
          <Show when={sheet().kind === "day"}>
            <DayDetail sheet={sheet()} onClose={() => setSheet(null)} />
          </Show>
          <Show when={sheet().kind === "model"}>
            <ModelDetail sheet={sheet()} metric={metric()} onClose={() => setSheet(null)} />
          </Show>
        </Sheet>
      </Show>
    </section>
  );

  async function openDay(date) {
    setSheet({ kind: "day", loading: true, date });
    try {
      const value = await getDay(date, agentParam());
      setSheet({ kind: "day", data: value, date });
    } catch (err) {
      setSheet({ kind: "day", error: err.message, date });
    }
  }

  async function openModel(model) {
    setSheet({ kind: "model", loading: true, model });
    try {
      const value = await getModel(model, report().range.from, report().range.to, agentParam());
      setSheet({ kind: "model", data: value, model });
    } catch (err) {
      setSheet({ kind: "model", error: err.message, model });
    }
  }
}

function Kpi(props) {
  return (
    <div class="kpi">
      <small>{props.label}</small>
      <strong>{props.value}</strong>
      <Show when={props.change !== null && props.change !== undefined}>
        <em classList={{ up: props.change > 0, down: props.change < 0 }}>{props.change > 0 ? "+" : ""}{props.change.toFixed(1)}%</em>
      </Show>
    </div>
  );
}

function ListSection(props) {
  return (
    <section class="list-section">
      <h2>{props.title}</h2>
      <Show when={props.rows.length} fallback={<div class="empty">{t("noData")}</div>}>
        <div class="list">
          <For each={props.rows.slice(0, props.limit)}>{(row) => props.render(row)}</For>
        </div>
        <Show when={props.rows.length > props.limit}>
          <button class="more" onClick={props.onMore}>{t("showMore")} ({props.rows.length - props.limit})</button>
        </Show>
      </Show>
    </section>
  );
}

function Heatmap(props) {
  const [step, setStep] = createSignal(null);
  let scrollRef;
  const report = () => props.report;
  const values = createMemo(() => {
    const map = new Map();
    for (const day of report().days || []) {
      map.set(day.date, props.metric === "cost" ? day.cost : day.tokens);
    }
    return map;
  });
  const from = () => report().range.from;
  const to = () => report().range.to;
  const max = createMemo(() => Math.max(1, ...values().values()));
  const thresholds = createMemo(() => [0.2, 0.4, 0.6, 0.8].map((part) => max() * part));
  const layout = () => report().layout || "week-rows";

  function level(value) {
    if (!value) return 0;
    const points = thresholds();
    if (value <= points[0]) return 1;
    if (value <= points[1]) return 2;
    if (value <= points[2]) return 3;
    if (value <= points[3]) return 4;
    return 5;
  }

  const dateList = createMemo(() => {
    const result = [];
    let cursor = parseDate(from());
    const end = parseDate(to());
    while (cursor <= end) {
      result.push(dateKey(cursor));
      cursor = addDays(cursor, 1);
    }
    return result;
  });

  const weeks = createMemo(() => {
    const start = parseDate(from());
    const end = parseDate(to());
    const monday = addDays(start, -((start.getDay() + 6) % 7));
    const days = [];
    let cursor = monday;
    while (cursor <= end || days.length % 7 !== 0) {
      days.push(dateKey(cursor));
      cursor = addDays(cursor, 1);
      if (days.length > 400) break;
    }
    return days;
  });

  const streaks = createMemo(() => {
    const set = new Set((report().days || []).map((day) => day.date));
    let longest = 0;
    let run = 0;
    let current = 0;
    const last = dateKey(new Date(Math.min(parseDate(to()).getTime(), Date.now())));
    for (const date of dateList()) {
      if (set.has(date)) {
        run += 1;
        longest = Math.max(longest, run);
        if (date <= last) current = run;
      } else {
        run = 0;
        current = 0;
      }
    }
    return { longest, current };
  });

  function cell(date) {
    const inside = date >= from() && date <= to();
    if (!inside) return <div class="heat-cell outside" />;
    const value = values().get(date) || 0;
    const active = step() === null || level(value) === step();
    return (
      <button
        class={`heat-cell h${level(value)}`}
        classList={{ dim: !active }}
        title={`${date} · ${props.metric === "cost" ? formatCost(value) : formatNumber(value)}`}
        onClick={() => value && props.report.days.find((day) => day.date === date) && props.onDay?.(date)}
      />
    );
  }

  return (
    <section class="heatmap-card">
      <div class="section-head">
        <h2>{t("heatmap")}</h2>
        <div class="streaks">
          <span>{t("streakCurrent")}: <strong>{streaks().current}</strong></span>
          <span>{t("streakLongest")}: <strong>{streaks().longest}</strong></span>
        </div>
      </div>
      <Show when={layout() === "strip"}>
        <div class="heat-strip">
          <For each={dateList()}>
            {(date) => (
              <div class="heat-strip-cell">
                {cell(date)}
                <small>{new Date(parseDate(date)).toLocaleDateString(locale(), { weekday: "narrow" })}</small>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={layout() === "week-rows"}>
        <div class="heat-week-rows">
          <div class="heat-week-head">
            <For each={[0, 1, 2, 3, 4, 5, 6]}>
              {(weekday) => <small>{new Date(2024, 0, 1 + weekday).toLocaleDateString(locale(), { weekday: "narrow" })}</small>}
            </For>
          </div>
          <For each={Array.from({ length: Math.ceil(weeks().length / 7) }, (_, index) => weeks().slice(index * 7, index * 7 + 7))}>
            {(week) => (
              <div class="heat-week-row">
                <For each={week}>{(date) => cell(date)}</For>
              </div>
            )}
          </For>
        </div>
      </Show>
      <Show when={layout() === "week-columns"}>
        <div class="heat-scroll" ref={(element) => {
          scrollRef = element;
          queueMicrotask(() => { if (scrollRef) scrollRef.scrollLeft = scrollRef.scrollWidth; });
        }}>
          <div class="heat-columns">
            <For each={Array.from({ length: Math.ceil(weeks().length / 7) }, (_, index) => weeks().slice(index * 7, index * 7 + 7))}>
              {(week) => (
                <div class="heat-column">
                  <For each={week}>{(date) => cell(date)}</For>
                </div>
              )}
            </For>
          </div>
        </div>
      </Show>
      <div class="heat-legend">
        <span>{t("less")}</span>
        <button classList={{ legend: true, active: step() === null }} onClick={() => setStep(null)} title={t("noData")} />
        <For each={[1, 2, 3, 4, 5]}>
          {(value) => (
            <button
              class={`legend h${value}`}
              classList={{ active: step() === value }}
              onClick={() => setStep(value)}
              title={rangeLabel(value)}
            />
          )}
        </For>
        <span>{t("more")}</span>
        <Show when={step() !== null}><small class="legend-range">{rangeLabel(step())}</small></Show>
      </div>
    </section>
  );

  function rangeLabel(value) {
    const points = thresholds();
    if (value === 0) return "0";
    const low = value === 1 ? 0 : points[value - 2];
    const high = points[value - 1];
    return `${formatNumber(low)} – ${formatNumber(high)}`;
  }
}

function Sheet(props) {
  return (
    <div class="sheet-backdrop" onClick={(event) => { if (event.target === event.currentTarget) props.onClose(); }}>
      <div class="sheet">
        <div class="sheet-handle" />
        {props.children}
      </div>
    </div>
  );
}

function DayDetail(props) {
  return (
    <>
      <div class="sheet-head">
        <h2>{props.sheet.date}</h2>
        <button class="icon-button" onClick={props.onClose}>×</button>
      </div>
      <Show when={props.sheet.loading}><div class="center"><div class="spinner" /></div></Show>
      <Show when={props.sheet.error}><div class="error-box">{props.sheet.error}</div></Show>
      <Show when={props.sheet.data}>
        <div class="detail-totals">
          <span>{formatNumber(props.sheet.data.totals.tokens)} {t("tokens")}</span>
          <span>{formatCost(props.sheet.data.totals.cost)}</span>
          <span>{t("cacheHit")} {formatPercent(cacheHit(props.sheet.data.totals), 2)}</span>
        </div>
        <For each={props.sheet.data.agents || []}>
          {(agent) => (
            <div class="detail-block">
              <h3>{agentLabel(agent.agent)}</h3>
              <table>
                <thead><tr><th>{t("model")}</th><th>{t("tokens")}</th><th>{t("cost")}</th></tr></thead>
                <tbody>
                  <For each={agent.models || []}>
                    {(model) => (
                      <tr>
                        <td>{model.model}<small>{model.company}</small></td>
                        <td>{formatNumber(model.tokens)}</td>
                        <td>{formatCost(model.cost)}</td>
                      </tr>
                    )}
                  </For>
                </tbody>
              </table>
            </div>
          )}
        </For>
      </Show>
    </>
  );
}

function ModelDetail(props) {
  const max = () => Math.max(1, ...(props.sheet.data?.perDay || []).map((day) => props.metric === "cost" ? day.cost : day.tokens));
  return (
    <>
      <div class="sheet-head">
        <h2>{props.sheet.model}</h2>
        <button class="icon-button" onClick={props.onClose}>×</button>
      </div>
      <Show when={props.sheet.loading}><div class="center"><div class="spinner" /></div></Show>
      <Show when={props.sheet.error}><div class="error-box">{props.sheet.error}</div></Show>
      <Show when={props.sheet.data}>
        <div class="detail-totals">
          <span>{formatNumber(props.sheet.data.totals.tokens)} {t("tokens")}</span>
          <span>{formatCost(props.sheet.data.totals.cost)}</span>
          <span>{t("lastUsed")}: {props.sheet.data.lastUsed}</span>
        </div>
        <div class="bars">
          <For each={props.sheet.data.perDay || []}>
            {(day) => (
              <div class="bar" title={`${day.date} · ${formatNumber(day.tokens)}`}>
                <i style={{ height: `${Math.max(2, ((props.metric === "cost" ? day.cost : day.tokens) / max()) * 100)}%` }} />
              </div>
            )}
          </For>
        </div>
        <table>
          <thead><tr><th>{t("agentsUsed")}</th><th>{t("tokens")}</th><th>{t("cost")}</th></tr></thead>
          <tbody>
            <For each={props.sheet.data.agents || []}>
              {(agent) => (
                <tr>
                  <td>{agentLabel(agent.agent)}</td>
                  <td>{formatNumber(agent.tokens)}</td>
                  <td>{formatCost(agent.cost)}</td>
                </tr>
              )}
            </For>
          </tbody>
        </table>
      </Show>
    </>
  );
}

function QuotaPage(props) {
  const [quota, setQuota] = createSignal(null);
  const [error, setError] = createSignal("");
  const [loading, setLoading] = createSignal(false);

  async function load() {
    setLoading(true);
    setError("");
    try {
      setQuota(await getQuota());
    } catch (err) {
      setError(err.message || t("error"));
    } finally {
      setLoading(false);
    }
  }
  createEffect(() => { props.nonce; load(); });

  const settings = () => props.state?.settings || {};
  const showEmpty = () => Boolean(settings()["display.show_empty_providers"]);
  const providers = () => (quota()?.providers || []).filter((provider) => provider.enabled || showEmpty());

  return (
    <section class="page">
      <Show when={error()}>
        <div class="error-box"><span>{t("error")}: {error()}</span><button onClick={load}>{t("retry")}</button></div>
      </Show>
      <Show when={loading() && !quota()}><div class="center"><div class="spinner" /></div></Show>
      <Show when={quota() && !providers().length}><div class="empty">{t("noQuota")}</div></Show>
      <For each={providers()}>
        {(provider) => <QuotaCard provider={provider} settings={settings()} />}
      </For>
    </section>
  );
}

function QuotaCard(props) {
  const snapshot = () => props.provider.snapshot;
  const accountDisplay = () => props.settings["display.account_display"] || "full";
  const remaining = () => props.settings["display.quota_display_mode"] === "remaining";
  const windows = () => snapshot()?.windows || [];
  return (
    <article classList={{ "quota-card": true, disabled: !props.provider.enabled }}>
      <div class="quota-head">
        <div>
          <h2>{agentLabel(props.provider.provider)}</h2>
          <Show when={props.provider.enabled === false}><small class="badge">{t("disabled")}</small></Show>
        </div>
        <Show when={snapshot()}>
          <div class="quota-meta">
            <Show when={accountDisplay() !== "none" && snapshot().plan}><small>{t("plan")}: {snapshot().plan}</small></Show>
            <Show when={accountDisplay() === "full" && snapshot().account}><small>{snapshot().account}</small></Show>
          </div>
        </Show>
      </div>
      <Show when={props.provider.error}>
        <p class="quota-error">{props.provider.error.message}</p>
      </Show>
      <Show when={!snapshot() && !props.provider.error}>
        <p class="note">{t("noQuota")}</p>
      </Show>
      <Show when={snapshot()}>
        <For each={windows()}>
          {(window) => <QuotaWindow window={window} remaining={remaining()} />}
        </For>
        <Show when={snapshot().extraUsage || snapshot().credits}>
          <div class="quota-extra">
            <small>{t("extraUsage")}</small>
            <strong>{formatCost((snapshot().extraUsage || snapshot().credits).used)} / {(snapshot().extraUsage || snapshot().credits).limit ? formatCost((snapshot().extraUsage || snapshot().credits).limit) : "—"}</strong>
          </div>
        </Show>
        <Show when={snapshot().rateLimitResetCredits?.length}>
          <div class="quota-extra">
            <small>{t("resetCredits")}</small>
            <strong>{snapshot().rateLimitResetCredits.length}</strong>
          </div>
        </Show>
      </Show>
    </article>
  );
}

function QuotaWindow(props) {
  const used = () => Math.max(0, Math.min(100, props.window.usedPercent || 0));
  const shown = () => props.remaining ? 100 - used() : used();
  const level = () => used() >= 100 ? "danger" : used() >= 80 ? "warn" : "normal";
  const pace = () => {
    if (used() <= 0 || used() >= 100 || !props.window.periodMs || !props.window.resetsAt) return null;
    const reset = new Date(props.window.resetsAt).getTime();
    const period = props.window.periodMs;
    const remainingMs = reset - Date.now();
    const elapsed = period - remainingMs;
    if (elapsed <= 0 || remainingMs <= 0) return null;
    const expected = (elapsed / period) * 100;
    const deficit = used() - expected;
    if (Math.abs(deficit) < 5) return { kind: "on_track", label: t("onTrack") };
    if (deficit > 0) return { kind: "ahead", label: t("paceAhead") };
    return { kind: "behind", label: t("paceBehind") };
  };
  return (
    <div class="quota-window">
      <div class="quota-window-head">
        <span>{props.window.label}</span>
        <strong>{formatPercent(shown(), 1)} <small>{props.remaining ? t("remaining") : t("used")}</small></strong>
      </div>
      <div class={`bar-track ${level()}`}><i style={{ width: `${shown()}%` }} /></div>
      <div class="quota-window-foot">
        <small>{props.window.resetsAt ? `${t("resetsIn")} ${formatReset(props.window.resetsAt)}` : ""}</small>
        <Show when={pace()}>
          <small class={`pace ${pace().kind}`}>{pace().label}</small>
        </Show>
      </div>
    </div>
  );
}

function KeeperPage(props) {
  const [keeper, setKeeper] = createSignal(null);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal("");
  const [wakeup, setWakeup] = createSignal({});

  async function load() {
    setError("");
    try {
      const value = await getKeeper();
      setKeeper(value);
      const times = {};
      for (const [id, agent] of Object.entries(value.agents || {})) times[id] = agent.dailyWakeupTime || "10:30";
      setWakeup(times);
    } catch (err) {
      setError(err.message || t("error"));
    }
  }
  createEffect(() => { props.nonce; load(); });

  async function update(key, value) {
    try {
      await patchSettings({ [key]: value });
      props.onSettings({ [key]: value });
      props.toast(t("settingsSaved"));
      load();
    } catch (err) {
      props.toast(err.message);
    }
  }

  async function ping(id, group) {
    setBusy(id + (group || ""));
    try {
      await pingKeeper(id, group);
      props.toast(t("pingSent"));
      setTimeout(load, 700);
    } catch (err) {
      props.toast(err.message);
    } finally {
      setBusy("");
    }
  }

  return (
    <section class="page">
      <Show when={error()}><div class="error-box"><span>{error()}</span><button onClick={load}>{t("retry")}</button></div></Show>
      <Show when={keeper()}>
        <div class="card engine-row">
          <div>
            <strong>{t("engine")}</strong>
            <small>{keeper().engine ? t("on") : t("off")}</small>
          </div>
          <button classList={{ toggle: true, on: keeper().engine }} onClick={async () => { await update("keeper.enabled", !keeper().engine); }}><i /></button>
        </div>
        <Show when={!keeper().engine}><p class="note">{t("keeperEngineOff")}</p></Show>
        <For each={Object.entries(keeper().agents || {})}>
          {([id, agent]) => (
            <article class="keeper-card">
              <div class="quota-head">
                <h2>{agentLabel(id)}</h2>
                <Show when={agent.running}><small class="badge running">{t("running")}</small></Show>
              </div>
              <div class="keeper-row">
                <span>{t("sessionKeeper")}</span>
                <button classList={{ toggle: true, on: agent.sessionKeeperEnabled }} onClick={() => update(`keeper.agents.${id}.session_keeper_enabled`, !agent.sessionKeeperEnabled)}><i /></button>
              </div>
              <div class="keeper-row">
                <span>{t("weeklyKeeper")}</span>
                <button classList={{ toggle: true, on: agent.weeklyKeeperEnabled }} onClick={() => update(`keeper.agents.${id}.weekly_keeper_enabled`, !agent.weeklyKeeperEnabled)}><i /></button>
              </div>
              <Show when={id === "agy"}>
                <div class="keeper-row">
                  <span>{t("weeklyAllGroups")}</span>
                  <button classList={{ toggle: true, on: agent.weeklyAllGroups }} onClick={() => update("keeper.agents.agy.weekly_all_groups", !agent.weeklyAllGroups)}><i /></button>
                </div>
              </Show>
              <div class="keeper-row">
                <span>{t("dailyWakeup")}</span>
                <input
                  type="time"
                  value={wakeup()[id] || agent.dailyWakeupTime || "10:30"}
                  onInput={(event) => setWakeup({ ...wakeup(), [id]: event.currentTarget.value })}
                  onChange={(event) => update(`keeper.agents.${id}.daily_wakeup_time`, event.currentTarget.value)}
                />
              </div>
              <div class="chips">
                <button class="primary small" disabled={busy() === id} onClick={() => ping(id)}>{busy() === id ? t("running") : t("ping")}</button>
                <For each={agent.targets || []}>
                  {(target) => (
                    <Show when={target.group}>
                      <button class="chip" disabled={busy() === id + target.group} onClick={() => ping(id, target.group)}>
                        {target.group} · {t("ping")}
                      </button>
                    </Show>
                  )}
                </For>
              </div>
            </article>
          )}
        </For>
        <section class="list-section">
          <h2>{t("recent")}</h2>
          <Show when={(keeper().executions || []).length} fallback={<div class="empty">{t("noExecutions")}</div>}>
            <div class="list">
              <For each={keeper().executions}>
                {(execution) => (
                  <div class="list-row static">
                    <div>
                      <strong>{agentLabel(execution.agent)} · {execution.trigger}</strong>
                      <small>{formatDateTime(execution.time)} · {execution.model}</small>
                      <Show when={execution.reply}><p class="reply">{execution.reply}</p></Show>
                    </div>
                    <span class={`status ${execution.status}`}>{execution.status === "success" ? t("success") : t("failed")}</span>
                  </div>
                )}
              </For>
            </div>
          </Show>
        </section>
      </Show>
    </section>
  );
}

function SettingsPage(props) {
  const [settings, setSettings] = createSignal(null);
  const [error, setError] = createSignal("");
  const [theme, setTheme] = createSignal(localStorage.getItem("tokenpulse-theme") || "system");
  const [push, setPush] = createSignal("loading");

  createEffect(() => {
    const value = theme();
    localStorage.setItem("tokenpulse-theme", value);
    if (value === "system") delete document.documentElement.dataset.theme;
    else document.documentElement.dataset.theme = value;
  });

  onMount(async () => {
    try {
      const value = await getSettings();
      setSettings(value.settings || {});
    } catch (err) {
      setError(err.message || t("error"));
    }
    if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
      setPush("unsupported");
    } else {
      const registration = await navigator.serviceWorker.getRegistration();
      const subscription = await registration?.pushManager.getSubscription();
      setPush(subscription ? "on" : "off");
    }
  });

  async function update(key, value) {
    try {
      const response = await patchSettings({ [key]: value });
      setSettings(response.settings || {});
      props.onSettings({ [key]: value });
      props.toast(t("settingsSaved"));
    } catch (err) {
      props.toast(err.message);
    }
  }

  const value = (key) => settings()?.[key]?.value;
  const allowed = (key) => settings()?.[key]?.allowed || [];

  async function enablePush() {
    try {
      const registration = await navigator.serviceWorker.register("/sw.js");
      const permission = await Notification.requestPermission();
      if (permission !== "granted") throw new Error(t("pushUnsupported"));
      const { publicKey } = await getPushKey();
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: base64Key(publicKey),
      });
      await addPushSubscription(subscription.toJSON());
      setPush("on");
      props.toast(t("pushEnabled"));
    } catch (err) {
      props.toast(err.message);
    }
  }

  async function disablePush() {
    const registration = await navigator.serviceWorker.getRegistration();
    const subscription = await registration?.pushManager.getSubscription();
    if (subscription) {
      await removePushSubscription(subscription.endpoint);
      await subscription.unsubscribe();
    }
    setPush("off");
  }

  function bool(key) {
    return (
      <button classList={{ toggle: true, on: Boolean(value(key)) }} onClick={() => update(key, !value(key))}><i /></button>
    );
  }

  function select(key, labels = {}) {
    return (
      <select value={value(key)} onChange={(event) => {
        const raw = event.currentTarget.value;
        const option = allowed(key).find((candidate) => String(candidate) === raw);
        update(key, option === undefined ? raw : option);
      }}>
        <For each={allowed(key)}>
          {(option) => <option value={String(option)}>{labels[String(option)] || String(option)}</option>}
        </For>
      </select>
    );
  }

  return (
    <section class="page">
      <Show when={error()}><div class="error-box">{error()}</div></Show>
      <Show when={settings()}>
        <div class="settings-group">
          <h2>{t("quota")}</h2>
          <Setting label={t("quotaDisplay")}>{select("display.quota_display_mode", { used: t("used"), remaining: t("remaining") })}</Setting>
          <Setting label={t("accountDisplay")}>{select("display.account_display")}</Setting>
          <Setting label={t("autoRefresh")}>{select("display.auto_refresh_secs", { 0: t("off") })}</Setting>
          <Setting label={t("refreshQuota")}>{bool("display.refresh_quota")}</Setting>
          <Setting label={t("scanAntigravity")}>{bool("display.scan_antigravity")}</Setting>
          <Setting label={t("showEmpty")}>{bool("display.show_empty_providers")}</Setting>
          <Setting label={t("notificationLevel")}>{select("display.notification_level")}</Setting>
          <Setting label={t("notificationSound")}>{select("display.notification_sound")}</Setting>
        </div>
        <div class="settings-group">
          <h2>{t("providers")}</h2>
          <Setting label="Claude">{bool("providers.claude.enabled")}</Setting>
          <Setting label="Codex">{bool("providers.codex.enabled")}</Setting>
          <Setting label="Antigravity">{bool("providers.agy.enabled")}</Setting>
        </div>
        <div class="settings-group">
          <h2>{t("keeper")}</h2>
          <Setting label={t("engine")}>{bool("keeper.enabled")}</Setting>
          <For each={["claude", "codex", "agy"]}>
            {(id) => (
              <>
                <h3>{agentLabel(id)}</h3>
                <Setting label={t("sessionKeeper")}>{bool(`keeper.agents.${id}.session_keeper_enabled`)}</Setting>
                <Setting label={t("weeklyKeeper")}>{bool(`keeper.agents.${id}.weekly_keeper_enabled`)}</Setting>
                <Show when={id === "agy"}>
                  <Setting label={t("weeklyAllGroups")}>{bool("keeper.agents.agy.weekly_all_groups")}</Setting>
                </Show>
                <Setting label={t("dailyWakeup")}>
                  <input
                    type="time"
                    value={value(`keeper.agents.${id}.daily_wakeup_time`) || "10:30"}
                    onChange={(event) => update(`keeper.agents.${id}.daily_wakeup_time`, event.currentTarget.value)}
                  />
                </Setting>
              </>
            )}
          </For>
        </div>
        <div class="settings-group">
          <h2>{t("theme")}</h2>
          <Setting label={t("theme")}>
            <select value={theme()} onChange={(event) => setTheme(event.currentTarget.value)}>
              <option value="system">{t("system")}</option>
              <option value="dark">{t("dark")}</option>
              <option value="light">{t("light")}</option>
            </select>
          </Setting>
          <Setting label={t("language")}><span>{isChinese() ? "中文" : "English"}</span></Setting>
        </div>
        <div class="settings-group">
          <h2>{t("push")}</h2>
          <Show when={push() === "unsupported"}><p class="note">{t("pushUnsupported")}</p></Show>
          <Show when={push() !== "unsupported"}>
            <button class={push() === "on" ? "secondary" : "primary"} onClick={push() === "on" ? disablePush : enablePush} disabled={push() === "loading"}>
              {push() === "on" ? t("pushDisable") : t("pushEnable")}
            </button>
          </Show>
        </div>
      </Show>
    </section>
  );
}

function Setting(props) {
  return <div class="settings-row"><span>{props.label}</span>{props.children}</div>;
}

function base64Key(value) {
  const padding = "=".repeat((4 - (value.length % 4)) % 4);
  const base64 = (value + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const output = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i += 1) output[i] = raw.charCodeAt(i);
  return output;
}

export default App;
