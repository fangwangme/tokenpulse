import { createEffect, createMemo, createSignal, For, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { createScrollNavHandler, state } from "../store";
import { request } from "../api";
import { scanIntervalToMs, USAGE_AGENT_KINDS as AGENT_KEYS } from "../shared/defaults";
import { cacheHitPercent, daysBetween, formatYMD, heatmapLayout, type UsagePreset } from "../shared/usage";
import { formatCost } from "../shared/money";
import type {
  UsageDayPayload,
  UsageDaySummary,
  UsageGetPayload,
  UsageModelPayload,
} from "../types";
import { FeatureDisabledNotice, formatModelName, Icon } from "./ui";
import { t, locale } from "../i18n";


const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  copilot: "Copilot",
  gemini: "Gemini",
  agy: "Antigravity",
  opencode: "OpenCode",
  pi: "Pi",
};

/** Only the agents serve currently enables are offered. */
function enabledAgentKeys(): readonly string[] {
  const listed = state.agents;
  if (!listed.length) return AGENT_KEYS;
  return AGENT_KEYS.filter((agent) => listed.includes(agent));
}

const COMPANY_COLORS: Record<string, string> = {
  anthropic: "var(--co-anthropic)",
  openai: "var(--co-openai)",
  google: "var(--co-google)",
  other: "var(--co-other)",
};

function formatWeekdayShort(day: number): string {
  const isZh = locale() === "zh";
  const zh = ["周日", "周一", "周二", "周三", "周四", "周五", "周六"];
  const en = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];
  return isZh ? zh[day] : en[day];
}

function getWeekdayTop(): string[] {
  const isZh = locale() === "zh";
  return isZh ? ["一", "二", "三", "四", "五", "六", "日"] : ["M", "T", "W", "T", "F", "S", "S"];
}

function getWeekdaySide(): string[] {
  const isZh = locale() === "zh";
  return isZh ? ["一", "", "三", "", "五", "", "日"] : ["M", "", "W", "", "F", "", "S"];
}

function formatMonthName(monthIdx: number): string {
  const isZh = locale() === "zh";
  const zh = ["1月", "2月", "3月", "4月", "5月", "6月", "7月", "8月", "9月", "10月", "11月", "12月"];
  const en = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
  return isZh ? zh[monthIdx] : en[monthIdx];
}

export function fmtTokens(n: number): string {
  if (n >= 1e9) return (n / 1e9).toFixed(2) + " B";
  if (n >= 1e6) return (n / 1e6).toFixed(2) + " M";
  if (n >= 1e3) return (n / 1e3).toFixed(2) + " K";
  return Math.round(n).toString();
}

export const fmtCost = formatCost;

export function fmtNumber(n: number): string {
  return Math.round(n).toLocaleString("en-US");
}

function todayYMD(): string {
  return formatYMD(new Date());
}

function SheetStatus(props: { error: string | null; onRetry: () => void }) {
  return (
    <Show when={props.error} fallback={<div style={{ padding: "20px 0", color: "var(--muted)" }}>{t("common.loading")}</div>}>
      <div
        style={{
          padding: "20px 0",
          display: "flex",
          "flex-direction": "column",
          "align-items": "flex-start",
          gap: "10px",
          color: "var(--muted)",
          "font-size": "13.5px",
        }}
      >
        <span>{props.error}</span>
        <button
          onClick={props.onRetry}
          style={{
            height: "34px",
            padding: "0 16px",
            "border-radius": "10px",
            border: 0,
            background: "var(--accent)",
            color: "var(--onaccent)",
            font: "inherit",
            "font-size": "13px",
            "font-weight": 600,
          }}
        >
          {t("usage.retry")}
        </button>
      </div>
    </Show>
  );
}

function UsageOverlay(props: { onClose: () => void; children: JSX.Element }) {
  return (
    <div data-overlay="" style={{ position: "fixed", inset: 0, "z-index": 50, background: "rgba(0,0,0,.5)" }}>
      <button
        onClick={props.onClose}
        aria-label={t("common.close")}
        style={{ position: "absolute", inset: 0, background: "none", border: 0 }}
      />
      {props.children}
    </div>
  );
}

function UsageBottomSheet(props: { onClose: () => void; maxHeight?: string; scrollable?: boolean; children: JSX.Element }) {
  return (
    <UsageOverlay onClose={props.onClose}>
      <div
        style={{
          position: "absolute",
          left: 0,
          right: 0,
          bottom: 0,
          background: "var(--surface)",
          "border-top": "1px solid var(--line2)",
          "border-radius": "24px 24px 0 0",
          padding: "10px 18px 40px",
          ...(props.maxHeight ? { "max-height": props.maxHeight } : {}),
          ...(props.scrollable ? { "overflow-y": "auto" } : {}),
        }}
      >
        <div style={{ width: "40px", height: "5px", "border-radius": "3px", background: "var(--line2)", margin: "0 auto 10px" }} />
        {props.children}
      </div>
    </UsageOverlay>
  );
}

export function UsagePage() {
  const [range, setRange] = createSignal<UsagePreset>(
    state.route.name === "usage" && state.route.initialPreset ? state.route.initialPreset : "today"
  );
  const [metric, setMetric] = createSignal<"tokens" | "cost">("tokens");
  const [group, setGroup] = createSignal<"date" | "model">("date");
  const [activeAgents, setActiveAgents] = createSignal<Set<string>>(new Set(AGENT_KEYS));
  createEffect(() => {
    if (!state.configReady) return;
    const enabled = enabledAgentKeys();
    if (enabled.length === 0) return;
    const current = activeAgents();
    const next = new Set([...current].filter((agent) => enabled.includes(agent)));
    if (next.size === 0) {
      for (const agent of enabled) next.add(agent);
    }
    if (next.size === current.size && [...next].every((agent) => current.has(agent))) return;
    setActiveAgents(next);
  });
  const [rowsLimit, setRowsLimit] = createSignal(10);

  const [customFrom, setCustomFrom] = createSignal("");
  const [customTo, setCustomTo] = createSignal("");
  const [draftFrom, setDraftFrom] = createSignal("");
  const [draftTo, setDraftTo] = createSignal("");

  const [usageReport, setUsageReport] = createSignal<UsageGetPayload | null>(null);
  const [loading, setLoading] = createSignal(state.usageEnabled && activeAgents().size > 0);
  const [reportError, setReportError] = createSignal<string | null>(null);
  const isLoading = () => loading();

  // Sheets
  const [sheet, setSheet] = createSignal<"none" | "day" | "model" | "custom" | "filter">("none");
  const [selectedDay, setSelectedDay] = createSignal<string | null>(null);
  const [dayData, setDayData] = createSignal<UsageDayPayload | null>(null);
  const [dayError, setDayError] = createSignal<string | null>(null);
  const [selectedModel, setSelectedModel] = createSignal<string | null>(null);
  const [modelData, setModelData] = createSignal<UsageModelPayload | null>(null);
  const [modelError, setModelError] = createSignal<string | null>(null);

  let usageReqSeq = 0;
  let dayReqSeq = 0;
  let modelReqSeq = 0;

  const HEAT_CELL = 24;
  const [selectedLegendBucket, setSelectedLegendBucket] = createSignal<number | null>(null);

  createEffect(() => {
    metric();
    range();
    setSelectedLegendBucket(null);
  });


  const loadUsage = async (force = false) => {
    if (!state.usageEnabled || activeAgents().size === 0) return;
    const agents = Array.from(activeAgents());
    const seq = ++usageReqSeq;
    setLoading(true);
    setReportError(null);
    try {
      const res = (await request({
        type: "usage_get",
        payload: {
          preset: range(),
          from: range() === "custom" ? customFrom() : undefined,
          to: range() === "custom" ? customTo() : undefined,
          agents,
          force,
        },
      })) as UsageGetPayload;
      if (seq !== usageReqSeq) return;
      setUsageReport(res);
      if (!customFrom() && res.firstDay) {
        setCustomFrom(res.firstDay);
        setCustomTo(todayYMD());
        setDraftFrom(res.firstDay);
        setDraftTo(todayYMD());
      }
    } catch (err: any) {
      console.error("[TokenPulse] usage_get error", err);
      if (seq === usageReqSeq) {
        setReportError(err?.message || t("usage.loadFailed"));
      }
    } finally {
      if (seq === usageReqSeq) {
        setLoading(false);
      }
    }
  };

  createEffect(() => {
    if (!state.usageEnabled || activeAgents().size === 0) return;
    // Re-fetch when range or activeAgents change
    const p = range();
    activeAgents(); // tracked: a filter change reloads
    const cf = customFrom();
    const ct = customTo();
    if (p !== "custom" || (cf && ct)) {
      void loadUsage();
    }
  });

  createEffect(
    on(
      () => state.usageScannedAt,
      (scannedAt) => {
        if (!scannedAt || !state.configReady || !state.usageEnabled) return;
        const rep = usageReport();
        if (rep && rep.scannedAt >= scannedAt) return;
        if (range() !== "custom" || (customFrom() && customTo())) {
          void loadUsage();
        }
      },
      { defer: true }
    )
  );

  createEffect(
    on(
      () => state.reconnectNonce,
      () => {
        if (state.configReady && state.usageEnabled && activeAgents().size > 0) void loadUsage();
      },
      { defer: true },
    ),
  );

  const checkStaleRefresh = () => {
    if (!state.connected || !state.configReady || !state.usageEnabled) return;
    if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
    if (loading()) return;
    if (range() === "custom" && (!customFrom() || !customTo())) return;
    const rep = usageReport();
    if (!rep) return;
    const staleMs = scanIntervalToMs(state.usageScanInterval) + 30_000;
    if (Date.now() - rep.scannedAt > staleMs) {
      void loadUsage();
    }
  };

  onMount(() => {
    checkStaleRefresh();
    const timer = setInterval(checkStaleRefresh, 30_000);
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", checkStaleRefresh);
    }
    onCleanup(() => {
      clearInterval(timer);
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", checkStaleRefresh);
      }
    });
  });

  const openDaySheet = async (date: string) => {
    const seq = ++dayReqSeq;
    setSelectedDay(date);
    setSheet("day");
    setDayData(null);
    setDayError(null);
    try {
      const res = (await request({
        type: "usage_day",
        payload: { date, agents: Array.from(activeAgents()) },
      })) as UsageDayPayload;
      if (seq !== dayReqSeq) return;
      setDayData(res);
    } catch (err: any) {
      console.error("[TokenPulse] usage_day error", err);
      if (seq === dayReqSeq) {
        setDayError(err?.message || t("usage.loadFailed"));
      }
    }
  };

  const rangeSpan = createMemo(() => {
    const rep = usageReport();
    if (!rep) return { from: todayYMD(), to: todayYMD(), len: 1 };
    const len = daysBetween(rep.range.from, rep.range.to) + 1;
    return { from: rep.range.from, to: rep.range.to, len };
  });

  const openModelSheet = async (model: string) => {
    const seq = ++modelReqSeq;
    setSelectedModel(model);
    setSheet("model");
    setModelData(null);
    setModelError(null);
    try {
      const res = (await request({
        type: "usage_model",
        payload: {
          model,
          from: rangeSpan().from,
          to: rangeSpan().to,
          agents: Array.from(activeAgents()),
        },
      })) as UsageModelPayload;
      if (seq !== modelReqSeq) return;
      setModelData(res);
    } catch (err: any) {
      console.error("[TokenPulse] usage_model error", err);
      if (seq === modelReqSeq) {
        setModelError(err?.message || t("usage.loadFailed"));
      }
    }
  };

  createEffect(
    on(
      () => state.configReady,
      (ready) => {
        if (!ready || !state.usageEnabled) return;
        if (sheet() === "model" && selectedModel() && !modelData()) {
          void openModelSheet(selectedModel()!);
        } else if (sheet() === "day" && selectedDay() && !dayData()) {
          void openDaySheet(selectedDay()!);
        }
      },
      { defer: true }
    )
  );

  const isSingleDay = () => rangeSpan().len === 1;

  const rangeText = () => {
    const span = rangeSpan();
    if (span.len === 1) {
      const [y, m, d] = span.from.split("-").map(Number);
      const dt = new Date(y, m - 1, d);
      return `${span.from} ${formatWeekdayShort(dt.getDay())}`;
    }
    return `${span.from} → ${span.to} · ${t("usage.rangeDays", { count: span.len.toString() })}`;
  };

  // KPI comparison deltas
  const kpis = createMemo(() => {
    const rep = usageReport();
    const cur = rep?.totals ?? { cost: 0, tokens: 0, sessions: 0 };
    const prev = rep?.comparison?.totals;

    const calcDelta = (c: number, p?: number) => {
      if (p === undefined) return { text: "—", color: "var(--muted)" };
      if (p <= 0) {
        if (c > 0) return { text: "+100%", color: "var(--danger)" };
        return { text: "0.0%", color: "var(--muted)" };
      }
      const pct = ((c - p) / p) * 100;
      if (Math.abs(pct) < 0.1) return { text: "0.0%", color: "var(--muted)" };
      const sign = pct > 0 ? "+" : "";
      return {
        text: `${sign}${pct.toFixed(1)}%`,
        color: pct > 0 ? "var(--danger)" : "var(--addfg)",
      };
    };

    return [
      {
        label: t("usage.cost"),
        value: fmtCost(cur.cost),
        color: "var(--fig-cost)",
        delta: calcDelta(cur.cost, prev?.cost),
      },
      {
        label: t("usage.tokens"),
        value: fmtTokens(cur.tokens),
        color: "var(--fg)",
        delta: calcDelta(cur.tokens, prev?.tokens),
      },
      {
        label: t("usage.sessions"),
        value: fmtNumber(cur.sessions),
        color: "var(--fg)",
        delta: calcDelta(cur.sessions, prev?.sessions),
      },
    ];
  });

  const comparisonNote = () => {
    const rep = usageReport();
    if (!rep) return "";
    const p = range();
    if (p === "year" || p === "all") return t("usage.noComparison");
    if (!rep.comparison) return t("usage.noPrevData");

    const comp = rep.comparison;
    const [y, m, d] = comp.from.split("-").map(Number);
    const dt = new Date(y, m - 1, d);
    const dayName = formatWeekdayShort(dt.getDay());
    const dateStr = `${String(m).padStart(2, "0")}-${String(d).padStart(2, "0")}`;

    if (rangeSpan().len === 1) {
      if (rangeSpan().from === todayYMD()) {
        const now = new Date();
        const timeStr = `${String(now.getHours()).padStart(2, "0")}:${String(now.getMinutes()).padStart(2, "0")}`;
        return t("usage.compareSameTime", { day: dayName, date: dateStr, time: timeStr });
      }
      return t("usage.compareFullDay", { day: dayName, date: dateStr });
    }
    return t("usage.compareEqualLength", { days: rangeSpan().len.toString() });
  };

  // Heatmap generation
  const heatmapData = createMemo(() => {
    const rep = usageReport();
    if (!rep || isSingleDay()) return null;

    const span = rangeSpan();
    const layout = heatmapLayout({ from: span.from, to: span.to });

    // Map of date string to day summary
    const dayMap = new Map<string, UsageDaySummary>();
    for (const d of rep.days) dayMap.set(d.date, d);

    const m = metric();
    let vmax = 0;
    for (const d of rep.days) {
      const val = m === "tokens" ? d.tokens : d.cost;
      if (val > vmax) vmax = val;
    }

    // Active days & streaks
    let activeDays = 0;
    let longestStreak = 0;
    let currentStreak = 0;
    let curRun = 0;

    // Check all dates in span
    const [y1, m1, d1] = span.from.split("-").map(Number);
    const [y2, m2, d2] = span.to.split("-").map(Number);
    const startDt = new Date(y1, m1 - 1, d1);
    const endDt = new Date(y2, m2 - 1, d2);

    for (let cur = new Date(startDt); cur <= endDt; cur.setDate(cur.getDate() + 1)) {
      const ymd = formatYMD(cur);
      const sum = dayMap.get(ymd);
      const val = sum ? (m === "tokens" ? sum.tokens : sum.cost) : 0;
      if (val > 0) {
        activeDays++;
        curRun++;
        if (curRun > longestStreak) longestStreak = curRun;
      } else {
        curRun = 0;
      }
    }

    // Current streak from end backwards
    for (let cur = new Date(endDt); cur >= startDt; cur.setDate(cur.getDate() - 1)) {
      const ymd = formatYMD(cur);
      const sum = dayMap.get(ymd);
      const val = sum ? (m === "tokens" ? sum.tokens : sum.cost) : 0;
      if (val > 0) currentStreak++;
      else break;
    }

    // Heat color helper
    const getCellColor = (val: number) => {
      if (val <= 0) return "var(--heat-empty)";
      const f = vmax > 0 ? val / vmax : 0;
      const step = f <= 0.2 ? 1 : f <= 0.4 ? 2 : f <= 0.6 ? 3 : f <= 0.8 ? 4 : 5;
      return `var(--heat-${m}-${step})`;
    };

    const getCellTextColor = (val: number) => {
      if (val <= 0) return "var(--muted)";
      const f = vmax > 0 ? val / vmax : 0;
      return f <= 0.6 ? "#0B1A22" : "#FFFFFF";
    };

    // Strip layout (<= 7 days)
    if (layout === "strip") {
      const cells = [];
      for (let cur = new Date(startDt); cur <= endDt; cur.setDate(cur.getDate() + 1)) {
        const ymd = formatYMD(cur);
        const sum = dayMap.get(ymd);
        const val = sum ? (m === "tokens" ? sum.tokens : sum.cost) : 0;
        cells.push({
          date: ymd,
          dayNum: cur.getDate(),
          weekdayLabel: getWeekdayTop()[(cur.getDay() + 6) % 7],
          color: getCellColor(val),
          textColor: getCellTextColor(val),
          val,
        });
      }
      return { layout, cells, activeDays, currentStreak, longestStreak, vmax };
    }

    // Week rows (<= 42 days, Monday-to-Sunday rows) & Week columns (longer periods)
    const dow1 = (startDt.getDay() + 6) % 7;
    const firstMonday = new Date(y1, m1 - 1, d1 - dow1);
    const dow2 = (endDt.getDay() + 6) % 7;
    const lastSunday = new Date(y2, m2 - 1, d2 + (6 - dow2));

    const weeks = [];
    let currentWeekCells = [];
    let currentWeekDays: Date[] = [];

    for (let cur = new Date(firstMonday); cur <= lastSunday; cur.setDate(cur.getDate() + 1)) {
      const ymd = formatYMD(cur);
      const inRange = cur >= startDt && cur <= endDt;
      const sum = inRange ? dayMap.get(ymd) : undefined;
      const val = sum ? (m === "tokens" ? sum.tokens : sum.cost) : 0;

      currentWeekCells.push({
        date: ymd,
        dayNum: cur.getDate(),
        inRange,
        color: inRange ? getCellColor(val) : "transparent",
        textColor: getCellTextColor(val),
        val,
      });
      currentWeekDays.push(new Date(cur));

      if (currentWeekCells.length === 7) {
        let monthLabel = "";
        for (const d of currentWeekDays) {
          if (d >= startDt && d <= endDt && d.getDate() === 1) {
            monthLabel = formatMonthName(d.getMonth());
            break;
          }
        }
        if (!monthLabel && weeks.length === 0) {
          const ref = currentWeekDays.find((d) => d >= startDt && d <= endDt) ?? currentWeekDays[0];
          monthLabel = formatMonthName(ref.getMonth());
        }
        weeks.push({ month: monthLabel, cells: currentWeekCells });
        currentWeekCells = [];
        currentWeekDays = [];
      }
    }

    return { layout, weeks, activeDays, currentStreak, longestStreak, vmax };
  });

  const bucketRangeText = () => {
    const b = selectedLegendBucket();
    if (b === null) return "";
    const hm = heatmapData();
    const vmax = hm?.vmax ?? 0;
    const isCost = metric() === "cost";
    const fmt = (v: number) => (isCost ? fmtCost(v) : fmtTokens(v));

    if (b === 0) {
      return `${t("usage.levelEmpty")}: ${isCost ? "$0.00" : "0"}`;
    }

    if (vmax <= 0) {
      return `${t("usage.level", { level: b.toString() })}: ${isCost ? "$0.00" : "0"}`;
    }

    const low = b === 1 ? 0 : vmax * ((b - 1) * 0.2);
    const high = b === 5 ? vmax : vmax * (b * 0.2);

    return `${t("usage.level", { level: b.toString() })}: ${fmt(low)} – ${fmt(high)}`;
  };

  // Table rows: date rows or model rows
  const tableRows = createMemo(() => {
    const rep = usageReport();
    if (!rep) return [];
    const isModel = isSingleDay() || group() === "model";
    const m = metric();

    if (!isModel) {
      // Date rows (newest first, descending)
      const days = [...rep.days].sort((a, b) => b.date.localeCompare(a.date));
      let maxVal = 0;
      for (const d of days) {
        const val = m === "tokens" ? d.tokens : d.cost;
        if (val > maxVal) maxVal = val;
      }

      return days.map((d) => {
        const val = m === "tokens" ? d.tokens : d.cost;
        const [y, mth, dayN] = d.date.split("-").map(Number);
        const dt = new Date(y, mth - 1, dayN);
        const title = `${d.date} ${formatWeekdayShort(dt.getDay())}`;
        const barW = maxVal > 0 ? `${Math.min(100, (val / maxVal) * 100)}%` : "0%";

        return {
          id: d.date,
          type: "date" as const,
          title,
          titleColor: "var(--fg)",
          sub: t("usage.sessionCount", { count: d.sessions.toString() }),
          subColor: "var(--muted)",
          tokens: fmtTokens(d.tokens),
          cost: fmtCost(d.cost),
          barW,
          barColor: m === "tokens" ? "var(--heat-tokens-3)" : "var(--fig-cost)",
          onTap: () => openDaySheet(d.date),
        };
      });
    }

    // Model rows: sorted strictly by tokens descending
    let models = [...rep.models].sort((a, b) => b.tokens - a.tokens);
    let maxVal = 0;
    for (const mdl of models) {
      const val = m === "tokens" ? mdl.tokens : mdl.cost;
      if (val > maxVal) maxVal = val;
    }

    return models.map((mdl) => {
      const val = m === "tokens" ? mdl.tokens : mdl.cost;
      const agentsStr = mdl.agents.map((ag) => AGENT_NAMES[ag] || ag).join(", ");
      const sessionStr = t("usage.sessionCount", { count: mdl.sessions.toString() });
      const sub = agentsStr ? `${sessionStr} · ${agentsStr}` : sessionStr;
      const barW = maxVal > 0 ? `${Math.min(100, (val / maxVal) * 100)}%` : "0%";

      return {
        id: mdl.model,
        type: "model" as const,
        title: formatModelName(mdl.agents, mdl.model),
        titleColor: COMPANY_COLORS[mdl.company] || "var(--co-other)",
        sub,
        subColor: "var(--muted)",
        tokens: fmtTokens(mdl.tokens),
        cost: fmtCost(mdl.cost),
        barW,
        barColor: COMPANY_COLORS[mdl.company] || "var(--co-other)",
        onTap: () => openModelSheet(mdl.model),
      };
    });
  });

  const visibleRows = () => tableRows().slice(0, rowsLimit());
  const onScroll = createScrollNavHandler();
  const isFiltered = () => activeAgents().size < enabledAgentKeys().length;

  return (
    <div class="page">
      <div class="scroll" onScroll={onScroll}>
        <div class="tp-page-body">
          <Show when={state.usageEnabled} fallback={<FeatureDisabledNotice message={t("usage.disabledNotice")} />}>
          {/* Range Chips (horizontally scrollable, Custom after All) */}
          <div class="tp-chips-scroll">
            <For
              each={[
                { id: "today" as const, label: t("usage.today") },
                { id: "7d" as const, label: t("usage.7d") },
                { id: "30d" as const, label: t("usage.30d") },
                { id: "year" as const, label: t("usage.year") },
                { id: "all" as const, label: t("usage.all") },
                {
                  id: "custom" as const,
                  label:
                    range() === "custom" && customFrom() && customTo()
                      ? `${customFrom().slice(5)} ~ ${customTo().slice(5)}`
                      : t("usage.custom"),
                },
              ]}
            >
              {(r) => {
                const on = () => range() === r.id;
                return (
                  <button
                    class="tp-chip"
                    classList={{ active: on() }}
                    onClick={() => {
                      if (r.id === "custom") {
                        setDraftFrom(customFrom() || todayYMD());
                        setDraftTo(customTo() || todayYMD());
                        setSheet("custom");
                      } else {
                        setRange(r.id);
                      }
                    }}
                  >
                    {r.label}
                  </button>
                );
              }}
            </For>
          </div>

          {/* Date range (left) + Scanned timestamp (right) */}
          <div
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              gap: "10px",
              padding: "10px 2px 0",
              "font-size": "12px",
              color: "var(--muted)",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            <span style={{ overflow: "hidden", "text-overflow": "ellipsis", "white-space": "nowrap" }}>
              {rangeText()}
            </span>
            <Show when={isLoading() || usageReport()?.scannedAt}>
              <span
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "5px",
                  flex: "none",
                  color: "var(--faint)",
                  "white-space": "nowrap",
                }}
              >
                <span
                  style={{
                    width: "6px",
                    height: "6px",
                    "border-radius": "50%",
                    background: isLoading() ? "var(--accent)" : "var(--addfg)",
                    opacity: isLoading() ? 1 : 0.75,
                  }}
                />
                {isLoading()
                  ? (usageReport() ? t("quota.refreshing") : t("usage.loading"))
                  : t("quota.scannedTo", {
                      time: new Date(usageReport()!.scannedAt).toTimeString().slice(0, 8),
                    })}
              </span>
            </Show>
          </div>

          {/* Metric toggle & Agent Filter pill */}
          <div style={{ "margin-top": "10px", display: "flex", "align-items": "center", gap: "8px" }}>
            <div class="tp-seg-pill">
              <button
                classList={{ on: metric() === "tokens" }}
                onClick={() => setMetric("tokens")}
              >
                {t("usage.byTokens")}
              </button>
              <button
                classList={{ on: metric() === "cost" }}
                onClick={() => setMetric("cost")}
              >
                {t("usage.byCost")}
              </button>
            </div>

            <div style={{ flex: 1 }} />

            <button
              class="tp-filter-pill"
              classList={{ filtered: isFiltered() }}
              onClick={() => setSheet("filter")}
              aria-label={t("usage.filterTitle")}
            >
              <Icon name="sliders" sm />
              {activeAgents().size === enabledAgentKeys().length
                ? t("usage.filterAllAgents")
                : t("usage.filterAgentCount", {
                    count: activeAgents().size.toString(),
                    total: enabledAgentKeys().length.toString(),
                  })}
            </button>
          </div>

          {/* KPI Figure Cards */}
          <Show
            when={usageReport()}
            fallback={
              <div style={{ "margin-top": "10px", display: "grid", "grid-template-columns": "repeat(3, minmax(0, 1fr))", gap: "8px" }}>
                <For each={[t("usage.cost"), t("usage.tokens"), t("usage.sessions")]}>
                  {(label) => (
                    <div
                      style={{
                        background: "var(--surface)",
                        border: "1px solid var(--line)",
                        "border-radius": "14px",
                        padding: "10px 12px",
                        display: "flex",
                        "flex-direction": "column",
                        gap: "6px",
                      }}
                    >
                      <div style={{ "font-size": "12px", color: "var(--muted)" }}>{label}</div>
                      <div class="skel" style={{ width: "70%", height: "20px", margin: "2px 0" }} />
                      <Show when={!isSingleDay()}>
                        <div class="skel" style={{ width: "45%", height: "12px" }} />
                      </Show>
                    </div>
                  )}
                </For>
              </div>
            }
          >
            {(() => {
              const items = kpis();
              const heroIdx = metric() === "cost" ? 0 : 1;
              const hero = items[heroIdx] ?? items[0];
              const secondary = items.filter((_, idx) => idx !== heroIdx);
              return (
                <div class="tp-hero" style={{ "margin-top": "10px", padding: "16px 18px 14px" }}>
                  <div style={{ display: "flex", "align-items": "center", "justify-content": "space-between", gap: "8px" }}>
                    <span style={{ "font-size": "12.5px", "font-weight": 550, color: "var(--muted)" }}>{hero.label}</span>
                    <Show when={!isSingleDay() && hero.delta.text && hero.delta.text !== "—"}>
                      <span
                        class="tp-pill"
                        style={{
                          color: hero.delta.color,
                          background: `color-mix(in srgb, ${hero.delta.color} 14%, transparent)`,
                        }}
                      >
                        {hero.delta.text}
                      </span>
                    </Show>
                  </div>
                  <div
                    style={{
                      "font-size": "34px",
                      "font-weight": 780,
                      "letter-spacing": "-.035em",
                      "line-height": 1.12,
                      "margin-top": "4px",
                      color: hero.color,
                      "font-variant-numeric": "tabular-nums",
                    }}
                  >
                    {hero.value}
                  </div>
                  <div
                    style={{
                      "margin-top": "14px",
                      "padding-top": "12px",
                      "border-top": "1px solid var(--line)",
                      display: "grid",
                      "grid-template-columns": "repeat(2, minmax(0, 1fr))",
                      gap: "12px",
                    }}
                  >
                    <For each={secondary}>
                      {(k) => (
                        <div>
                          <div style={{ "font-size": "11.5px", color: "var(--muted)" }}>{k.label}</div>
                          <div style={{ display: "flex", "align-items": "baseline", gap: "6px", "margin-top": "2px" }}>
                            <span
                              style={{
                                "font-size": "17px",
                                "font-weight": 700,
                                "letter-spacing": "-.02em",
                                color: k.color,
                                "font-variant-numeric": "tabular-nums",
                              }}
                            >
                              {k.value}
                            </span>
                            <Show when={!isSingleDay() && k.delta.text && k.delta.text !== "—"}>
                              <span style={{ "font-size": "11px", color: k.delta.color, "font-variant-numeric": "tabular-nums" }}>
                                {k.delta.text}
                              </span>
                            </Show>
                          </div>
                        </div>
                      )}
                    </For>
                  </div>
                </div>
              );
            })()}
          </Show>

          {/* Comparison line (only when multi-day period and data loaded) */}
          <Show when={!isSingleDay() && usageReport()}>
            <div style={{ padding: "6px 4px 0", "font-size": "11.5px", color: "var(--faint)" }}>
              {comparisonNote()}
            </div>
          </Show>

          {/* Heatmap Card Skeleton when loading */}
          <Show when={!isSingleDay() && isLoading() && !usageReport()}>
            <div
              style={{
                "margin-top": "12px",
                background: "var(--surface)",
                border: "1px solid var(--line)",
                "border-radius": "18px",
                padding: "16px 14px",
                display: "flex",
                "flex-direction": "column",
                gap: "14px",
              }}
            >
              <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
                <div class="skel" style={{ width: "110px", height: "16px" }} />
                <div style={{ flex: 1 }} />
                <div class="skel" style={{ width: "90px", height: "14px" }} />
              </div>
              <div
                style={{
                  height: "90px",
                  display: "flex",
                  "flex-direction": "column",
                  "align-items": "center",
                  "justify-content": "center",
                  gap: "10px",
                  color: "var(--muted)",
                  "font-size": "13px",
                }}
              >
                <div class="spin" style={{ width: "18px", height: "18px", "border-width": "2px" }} />
                <span>{t("usage.loading")}</span>
              </div>
            </div>
          </Show>

          {/* Heatmap Card (if period > 1 day) */}
          <Show when={!isSingleDay() && heatmapData()}>
            {(() => {
              const hm = heatmapData()!;
              const mName = () => (metric() === "tokens" ? t("usage.tokens") : t("usage.cost"));

              return (
                <div
                  style={{
                    "margin-top": "12px",
                    background: "var(--surface)",
                    border: "1px solid var(--line)",
                    "border-radius": "18px",
                    padding: "14px 14px 12px",
                  }}
                >
                  {/* Heatmap Header */}
                  <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
                    <div style={{ flex: 1, "min-width": 0 }}>
                      <div style={{ "font-weight": 650, "font-size": "15px" }}>
                        {t("usage.dailyMetric", { metric: mName() })}
                      </div>
                      <div style={{ "font-size": "12px", color: "var(--muted)" }}>
                        {t("usage.heatStats", {
                          active: hm.activeDays.toString(),
                          current: hm.currentStreak.toString(),
                          longest: hm.longestStreak.toString(),
                        })}
                      </div>
                    </div>

                  </div>

                  {/* Strip layout */}
                  <Show when={hm.layout === "strip" && hm.cells}>
                    <div style={{ "margin-top": "12px" }}>
                      <div style={{ display: "flex", gap: "6px" }}>
                        <For each={hm.cells}>
                          {(c) => (
                            <div style={{ flex: 1, "text-align": "center", "font-size": "11px", color: "var(--faint)" }}>
                              {c.weekdayLabel}
                            </div>
                          )}
                        </For>
                      </div>
                      <div style={{ display: "flex", gap: "6px", "margin-top": "6px" }}>
                        <For each={hm.cells}>
                          {(c) => (
                            <button
                              onClick={() => {
                                if (c.val > 0) void openDaySheet(c.date);
                              }}
                              style={{
                                flex: 1,
                                height: "38px",
                                "border-radius": "6px",
                                border: 0,
                                background: c.color,
                                color: c.textColor,
                                font: "inherit",
                                "font-size": "11px",
                                "font-weight": 600,
                                "font-variant-numeric": "tabular-nums",
                                display: "flex",
                                "align-items": "flex-start",
                                "justify-content": "flex-start",
                                padding: "4px 0 0 5px",
                                "box-sizing": "border-box",
                              }}
                            >
                              {c.dayNum}
                            </button>
                          )}
                        </For>
                      </div>
                    </div>
                  </Show>

                  {/* Week-rows layout */}
                  <Show when={hm.layout === "week-rows" && hm.weeks}>
                    <div style={{ "margin-top": "12px" }}>
                      <div style={{ display: "flex", gap: "5px", "padding-left": "30px", "font-size": "10.5px", color: "var(--faint)" }}>
                        <For each={getWeekdayTop()}>
                          {(w) => <span style={{ flex: 1, "text-align": "center" }}>{w}</span>}
                        </For>
                      </div>
                      <For each={hm.weeks}>
                        {(w) => (
                          <div style={{ display: "flex", gap: "5px", "align-items": "center", "margin-top": "5px" }}>
                            <span style={{ width: "26px", "font-size": "10.5px", color: "var(--faint)", "white-space": "nowrap" }}>
                              {w.month}
                            </span>
                            <For each={w.cells}>
                              {(c) => (
                                <button
                                  onClick={() => {
                                    if (c.val > 0) void openDaySheet(c.date);
                                  }}
                                  style={{
                                    flex: 1,
                                    height: "36px",
                                    "border-radius": "6px",
                                    border: 0,
                                    background: c.color,
                                    color: c.textColor,
                                    font: "inherit",
                                    "font-size": "11px",
                                    "font-weight": 600,
                                    "font-variant-numeric": "tabular-nums",
                                    display: "flex",
                                    "align-items": "flex-start",
                                    "justify-content": "flex-start",
                                    padding: "3px 0 0 5px",
                                    "box-sizing": "border-box",
                                  }}
                                >
                                  {c.inRange ? c.dayNum : ""}
                                </button>
                              )}
                            </For>
                          </div>
                        )}
                      </For>
                    </div>
                  </Show>

                  {/* Week-columns layout */}
                  <Show when={hm.layout === "week-columns" && hm.weeks}>
                    {(() => {
                      return (
                        <div style={{ display: "flex", gap: "4px", "margin-top": "12px" }}>
                          <div
                            style={{
                              display: "flex",
                              "flex-direction": "column",
                              gap: "4px",
                              "font-size": "10px",
                              color: "var(--faint)",
                              width: "14px",
                              "padding-top": "16px",
                              flex: "none",
                            }}
                          >
                            <For each={getWeekdaySide()}>
                              {(wl) => (
                                <span style={{ height: `${HEAT_CELL}px`, "line-height": `${HEAT_CELL}px` }}>
                                  {wl}
                                </span>
                              )}
                            </For>
                          </div>

                          <div
                            style={{
                              flex: 1,
                              "min-width": 0,
                              "overflow-x": "auto",
                              display: "flex",
                              "flex-direction": "row-reverse",
                              "touch-action": "pan-x",
                            }}
                          >
                            <div style={{ display: "flex", gap: "4px", flex: "none", "padding-bottom": "2px" }}>
                              <For each={hm.weeks}>
                                {(w) => (
                                  <div style={{ display: "flex", "flex-direction": "column", gap: "4px" }}>
                                    <span
                                      style={{
                                        height: "14px",
                                        "font-size": "10px",
                                        color: "var(--faint)",
                                        "white-space": "nowrap",
                                        width: `${HEAT_CELL}px`,
                                      }}
                                    >
                                      {w.month}
                                    </span>
                                    <For each={w.cells}>
                                      {(c, rowIdx) => (
                                        <button
                                          onClick={() => {
                                            if (c.val > 0) void openDaySheet(c.date);
                                          }}
                                          style={{
                                            width: `${HEAT_CELL}px`,
                                            height: `${HEAT_CELL}px`,
                                            padding: "2px 0 0 3px",
                                            "border-radius": "5px",
                                            border: 0,
                                            background: c.color,
                                            color: c.textColor,
                                            font: "inherit",
                                            "font-size": "10px",
                                            "font-weight": 600,
                                            "font-variant-numeric": "tabular-nums",
                                            display: "flex",
                                            "align-items": "flex-start",
                                            "justify-content": "flex-start",
                                            "box-sizing": "border-box",
                                          }}
                                        >
                                          {rowIdx() === 0 && c.inRange ? c.dayNum : ""}
                                        </button>
                                      )}
                                    </For>
                                  </div>
                                )}
                              </For>
                            </div>
                          </div>
                        </div>
                      );
                    })()}
                  </Show>

                  {/* Heatmap Legend */}
                  <div
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "4px",
                      "margin-top": "10px",
                      "font-size": "11px",
                      color: "var(--faint)",
                      "flex-wrap": "wrap",
                    }}
                  >
                    <span
                      style={{
                        flex: 1,
                        "min-width": "120px",
                        "font-size": "11.5px",
                        "font-weight": 600,
                        color: "var(--fg)",
                        "font-variant-numeric": "tabular-nums",
                      }}
                    >
                      {bucketRangeText()}
                    </span>
                    <div style={{ display: "flex", "align-items": "center", gap: "3px", "margin-left": "auto" }}>
                      <span style={{ "margin-right": "3px" }}>{t("usage.less")}</span>
                      <button
                        type="button"
                        aria-label={t("usage.levelEmpty")}
                        onClick={() => setSelectedLegendBucket((c) => (c === 0 ? null : 0))}
                        style={{
                          width: "12px",
                          height: "12px",
                          padding: 0,
                          "border-radius": "2px",
                          border: selectedLegendBucket() === 0 ? "1.5px solid var(--fg)" : "1px solid var(--line)",
                          background: "var(--heat-empty)",
                          cursor: "pointer",
                          outline: "none",
                          transform: selectedLegendBucket() === 0 ? "scale(1.25)" : "none",
                          transition: "transform .15s, border-color .15s",
                        }}
                      />
                      <For each={[1, 2, 3, 4, 5]}>
                        {(lvl) => (
                          <button
                            type="button"
                            aria-label={t("usage.level", { level: lvl.toString() })}
                            onClick={() => setSelectedLegendBucket((c) => (c === lvl ? null : lvl))}
                            style={{
                              width: "12px",
                              height: "12px",
                              padding: 0,
                              "border-radius": "2px",
                              border: selectedLegendBucket() === lvl ? "1.5px solid var(--fg)" : "1px solid transparent",
                              background: `var(--heat-${metric()}-${lvl})`,
                              cursor: "pointer",
                              outline: "none",
                              transform: selectedLegendBucket() === lvl ? "scale(1.25)" : "none",
                              transition: "transform .15s, border-color .15s",
                            }}
                          />
                        )}
                      </For>
                      <span style={{ "margin-left": "3px" }}>{t("usage.more")}</span>
                    </div>
                  </div>
                </div>
              );
            })()}
          </Show>

          {/* List Card */}
          <div class="tp-card" style={{ "margin-top": "12px", padding: "14px 0 6px" }}>
            {/* List Header */}
            <div style={{ display: "flex", "align-items": "center", padding: "0 16px", "min-height": "34px" }}>
              <span style={{ "font-weight": 680, "font-size": "15px", flex: 1 }}>
                {isSingleDay() ? t("usage.byModel") : group() === "date" ? t("usage.byDate") : t("usage.byModel")}
              </span>
              <Show when={!isSingleDay()}>
                <div
                  style={{
                    display: "flex",
                    background: "var(--bg)",
                    border: "1px solid var(--line)",
                    "border-radius": "10px",
                    padding: "2px",
                  }}
                >
                  <button
                    onClick={() => setGroup("date")}
                    style={{
                      height: "28px",
                      padding: "0 11px",
                      "border-radius": "8px",
                      border: 0,
                      font: "inherit",
                      "font-size": "12.5px",
                      "font-weight": group() === "date" ? 600 : 450,
                      background: group() === "date" ? "var(--surface)" : "none",
                      color: group() === "date" ? "var(--fg)" : "var(--muted)",
                      "box-shadow": group() === "date" ? "0 1px 4px rgba(0,0,0,.12)" : "none",
                    }}
                  >
                    {t("usage.byDate")}
                  </button>
                  <button
                    onClick={() => setGroup("model")}
                    style={{
                      height: "28px",
                      padding: "0 11px",
                      "border-radius": "8px",
                      border: 0,
                      font: "inherit",
                      "font-size": "12.5px",
                      "font-weight": group() === "model" ? 600 : 450,
                      background: group() === "model" ? "var(--surface)" : "none",
                      color: group() === "model" ? "var(--fg)" : "var(--muted)",
                      "box-shadow": group() === "model" ? "0 1px 4px rgba(0,0,0,.12)" : "none",
                    }}
                  >
                    {t("usage.byModel")}
                  </button>
                </div>
              </Show>
            </div>

            {/* Company Share Pill Bar */}
            <Show when={usageReport() && usageReport()!.models.length > 0}>
              {(() => {
                const models = usageReport()!.models;
                const byCompany = new Map<string, number>();
                let sum = 0;
                for (const m of models) {
                  const val = metric() === "cost" ? m.cost : m.tokens;
                  if (val <= 0) continue;
                  const co = m.company || "other";
                  byCompany.set(co, (byCompany.get(co) || 0) + val);
                  sum += val;
                }
                if (sum <= 0) return null;
                const segs = Array.from(byCompany.entries())
                  .sort((a, b) => b[1] - a[1])
                  .map(([co, val]) => ({
                    co,
                    pct: (val / sum) * 100,
                    color: COMPANY_COLORS[co] || COMPANY_COLORS.other,
                  }));
                const coLabel = (co: string) =>
                  co === "anthropic" ? "Anthropic" : co === "openai" ? "OpenAI" : co === "google" ? "Google" : co;
                return (
                  <div style={{ padding: "10px 16px 8px" }}>
                    <div class="tp-share-bar">
                      <For each={segs}>
                        {(seg) => (
                          <div
                            class="tp-share-seg"
                            style={{ width: `${Math.max(2, seg.pct)}%`, background: seg.color }}
                          />
                        )}
                      </For>
                    </div>
                    <div style={{ display: "flex", "flex-wrap": "wrap", gap: "10px", "margin-top": "7px" }}>
                      <For each={segs}>
                        {(seg) => (
                          <span style={{ display: "inline-flex", "align-items": "center", gap: "5px", "font-size": "11.5px", color: "var(--muted)", "font-variant-numeric": "tabular-nums" }}>
                            <span style={{ width: "7px", height: "7px", "border-radius": "50%", background: seg.color }} />
                            <span>{coLabel(seg.co)}</span>
                            <b style={{ color: "var(--fg)", "font-weight": 600 }}>{Math.round(seg.pct)}%</b>
                          </span>
                        )}
                      </For>
                    </div>
                  </div>
                );
              })()}
            </Show>

            {/* Skeleton Rows when loading */}
            <Show when={isLoading() && !usageReport()}>
              <div style={{ "margin-top": "6px" }}>
                <For each={[1, 2, 3]}>
                  {() => (
                    <div
                      style={{
                        display: "flex",
                        "align-items": "center",
                        gap: "10px",
                        width: "100%",
                        "min-height": "58px",
                        padding: "10px 14px",
                        "border-top": "1px solid var(--line)",
                      }}
                    >
                      <div style={{ flex: 1, display: "flex", "flex-direction": "column", gap: "6px" }}>
                        <div class="skel" style={{ width: "42%", height: "14px" }} />
                        <div class="skel" style={{ width: "68%", height: "11px" }} />
                      </div>
                      <div style={{ display: "flex", "flex-direction": "column", "align-items": "flex-end", gap: "6px" }}>
                        <div class="skel" style={{ width: "56px", height: "14px" }} />
                        <div class="skel" style={{ width: "40px", height: "11px" }} />
                      </div>
                    </div>
                  )}
                </For>
              </div>
            </Show>

            {/* Error state when report failed to load */}
            <Show when={!isLoading() && !usageReport() && reportError()}>
              <div
                style={{
                  "border-top": "1px solid var(--line)",
                  "margin-top": "8px",
                  padding: "22px 14px",
                  display: "flex",
                  "flex-direction": "column",
                  "align-items": "center",
                  gap: "10px",
                  "text-align": "center",
                  color: "var(--muted)",
                  "font-size": "13.5px",
                }}
              >
                <span>{reportError()}</span>
                <button
                  onClick={() => void loadUsage()}
                  style={{
                    height: "34px",
                    padding: "0 16px",
                    "border-radius": "10px",
                    border: 0,
                    background: "var(--accent)",
                    color: "var(--onaccent)",
                    font: "inherit",
                    "font-size": "13px",
                    "font-weight": 600,
                  }}
                >
                  {t("usage.retry")}
                </button>
              </div>
            </Show>

            {/* Empty state (only when loading finished with a report and no rows) */}
            <Show when={!isLoading() && !!usageReport() && tableRows().length === 0}>
              <div
                style={{
                  "border-top": "1px solid var(--line)",
                  "margin-top": "8px",
                  padding: "22px 0",
                  "text-align": "center",
                  color: "var(--muted)",
                  "font-size": "13.5px",
                }}
              >
                {t("usage.empty")}
              </div>
            </Show>

            {/* Rows */}
            <Show when={usageReport()}>
              <div style={{ "margin-top": "6px" }}>
              <For each={visibleRows()}>
                {(r) => (
                  <button
                    onClick={r.onTap}
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "10px",
                      width: "100%",
                      "min-height": "58px",
                      padding: "8px 10px 8px 14px",
                      background: "none",
                      border: 0,
                      "border-top": "1px solid var(--line)",
                      color: "var(--fg)",
                      font: "inherit",
                      "text-align": "left",
                      "font-variant-numeric": "tabular-nums",
                    }}
                  >
                    <span style={{ flex: 1, "min-width": 0 }}>
                      <span
                        style={{
                          display: "block",
                          "font-size": "14.5px",
                          "font-weight": 600,
                          color: r.titleColor,
                          "overflow-wrap": "anywhere",
                          "line-height": 1.35,
                        }}
                      >
                        {r.title}
                      </span>
                      <span style={{ display: "flex", gap: "6px", "align-items": "baseline", "margin-top": "3px", "font-size": "12.5px" }}>
                        <span style={{ color: r.subColor, flex: 1, "min-width": 0, "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                          {r.sub}
                        </span>
                        <span
                          style={{
                            color: "var(--fig-cost)",
                            "font-weight": 600,
                            "white-space": "nowrap",
                            flex: "none",
                            "text-align": "right",
                            "font-variant-numeric": "tabular-nums",
                          }}
                        >
                          {r.cost}
                        </span>
                        <span
                          style={{
                            color: "var(--fg)",
                            "font-weight": 600,
                            "white-space": "nowrap",
                            flex: "none",
                            width: "56px",
                            "text-align": "right",
                            "font-variant-numeric": "tabular-nums",
                          }}
                        >
                          {r.tokens}
                        </span>
                      </span>
                      <span
                        style={{
                          display: "block",
                          height: "3px",
                          "border-radius": "2px",
                          background: "var(--line2)",
                          "margin-top": "6px",
                          overflow: "hidden",
                        }}
                      >
                        <span
                          style={{
                            display: "block",
                            height: "3px",
                            width: r.barW,
                            background: r.barColor,
                          }}
                        />
                      </span>
                    </span>
                    <Icon name="chevR" sm />
                  </button>
                )}
              </For>
            </div>

            {/* Pagination */}
            <Show when={tableRows().length > rowsLimit()}>
              <div style={{ "border-top": "1px solid var(--line)", padding: "12px", "text-align": "center" }}>
                <button
                  onClick={() => setRowsLimit((c) => c + 20)}
                  style={{
                    color: "var(--accent)",
                    "font-size": "13px",
                    "font-weight": 550,
                  }}
                >
                  {t("usage.showMoreRows", { count: (tableRows().length - rowsLimit()).toString() })}
                </button>
              </div>
            </Show>
          </Show>
          </div>

          {/* Bottom notes if unpriced */}
          <Show when={usageReport() && usageReport()!.models.some((m) => m.unpriced)}>
            <div style={{ padding: "12px 8px 0", "font-size": "12px", color: "var(--faint)" }}>
              {t("usage.unpricedNote")}
            </div>
          </Show>
          </Show>
        </div>
      </div>

      {/* Day Sheet */}
      <Show when={sheet() === "day" && selectedDay()}>
        <UsageBottomSheet onClose={() => setSheet("none")} maxHeight="85vh" scrollable>
          <div style={{ display: "flex", "align-items": "center" }}>
            <span style={{ "font-weight": 650, "font-size": "17px", flex: 1 }}>
              {selectedDay()}
            </span>
            <button
              class="cbtn"
              aria-label={t("common.close")}
              onClick={() => setSheet("none")}
            >
              <Icon name="x" />
            </button>
          </div>

            <Show when={dayData()} fallback={<SheetStatus error={dayError()} onRetry={() => selectedDay() && void openDaySheet(selectedDay()!)} />}>
              {(() => {
                const d = dayData()!;
                return (
                  <div>
                    {/* Metrics grid */}
                    <div
                      style={{
                        display: "grid",
                        "grid-template-columns": "repeat(2, minmax(0, 1fr))",
                        gap: "4px 14px",
                        "margin-top": "6px",
                        "font-size": "13px",
                        "font-variant-numeric": "tabular-nums",
                      }}
                    >
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cost")}</span><span style={{ color: "var(--fig-cost)", "font-weight": 600 }}>{fmtCost(d.totals.cost)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.tokens")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(d.totals.tokens)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.input")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(d.totals.input)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.output")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(d.totals.output)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheRead")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(d.totals.cacheRead)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheWrite")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{d.totals.cacheWrite > 0 ? fmtTokens(d.totals.cacheWrite) : "—"}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.messages")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtNumber(d.totals.messages)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.sessions")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtNumber(d.totals.sessions)}</span></div>
                    </div>

                    {/* Breakdown by Agent */}
                    <div style={{ "font-size": "12.5px", color: "var(--faint)", "margin-top": "12px" }}>
                      {t("usage.agentAndModel")}
                    </div>
                    <For each={d.agents}>
                      {(ag) => (
                        <div style={{ "border-top": "1px solid var(--line)", "margin-top": "8px", "padding-top": "8px" }}>
                          <div style={{ display: "flex", "align-items": "baseline" }}>
                            <span style={{ flex: 1, "font-weight": 650, color: `var(--agent-${ag.agent})` }}>
                              {AGENT_NAMES[ag.agent] || ag.agent}
                            </span>
                            <span style={{ "font-weight": 700, color: `var(--agent-${ag.agent})`, "font-variant-numeric": "tabular-nums" }}>
                              {fmtCost(ag.cost)}
                            </span>
                          </div>
                          <For each={ag.models}>
                            {(m) => (
                              <div style={{ padding: "4px 0 0 12px", "font-variant-numeric": "tabular-nums" }}>
                                <div style={{ display: "flex", gap: "8px", "font-size": "13px" }}>
                                  <span style={{ flex: 1, color: COMPANY_COLORS[m.company || "other"] || "var(--co-other)", "overflow-wrap": "anywhere" }}>
                                    {formatModelName(ag.agent, m.model)}
                                  </span>
                                  <span style={{ color: "var(--fig-cost)", "font-weight": 600 }}>{fmtCost(m.cost)}</span>
                                </div>
                                <div style={{ "font-size": "11.5px", display: "flex", "flex-wrap": "wrap", gap: "0 6px" }}>
                                  <span><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.tokens")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(m.tokens)}</span></span>
                                  <span style={{ color: "var(--faint)" }}>·</span>
                                  <span><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.input")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(m.input)}</span></span>
                                  <span style={{ color: "var(--faint)" }}>·</span>
                                  <span><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.output")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(m.output)}</span></span>
                                  <span style={{ color: "var(--faint)" }}>·</span>
                                  <span title={t("usage.cacheRead")}><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheReadShort")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(m.cacheRead)}</span></span>
                                  <span style={{ color: "var(--faint)" }}>·</span>
                                  <span title={t("usage.cacheWrite")}><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheWriteShort")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{m.cacheWrite > 0 ? fmtTokens(m.cacheWrite) : "—"}</span></span>
                                  <span style={{ color: "var(--faint)" }}>·</span>
                                  <span title={t("usage.cacheHit")}><span style={{ color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheHitShort")} </span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{cacheHitPercent(m.cacheRead, m.tokens).toFixed(2)}%</span></span>
                                </div>
                              </div>
                            )}
                          </For>
                        </div>
                      )}
                    </For>
                  </div>
                );
              })()}
            </Show>
        </UsageBottomSheet>
      </Show>

      {/* Model Sheet */}
      <Show when={sheet() === "model" && selectedModel()}>
        <UsageBottomSheet onClose={() => setSheet("none")} maxHeight="85vh" scrollable>
            <div style={{ display: "flex", "align-items": "flex-start", gap: "8px" }}>
              <div style={{ flex: 1, "min-width": 0 }}>
                <div
                  style={{
                    "font-weight": 700,
                    "font-size": "17px",
                    color: modelData() ? COMPANY_COLORS[modelData()!.company || "other"] || "var(--co-other)" : "var(--fg)",
                    "overflow-wrap": "anywhere",
                  }}
                >
                  {formatModelName(modelData()?.agents.map((a) => a.agent), selectedModel())}
                </div>
                <div style={{ "font-size": "12.5px", color: "var(--faint)" }}>
                  {modelData()?.company ?? ""} · {rangeText()}
                </div>
              </div>
              <button
                class="cbtn"
                aria-label={t("common.close")}
                onClick={() => setSheet("none")}
              >
                <Icon name="x" />
              </button>
            </div>

            <Show when={modelData()} fallback={<SheetStatus error={modelError()} onRetry={() => selectedModel() && void openModelSheet(selectedModel()!)} />}>
              {(() => {
                const mdl = modelData()!;
                const col = COMPANY_COLORS[mdl.company || "other"] || "var(--co-other)";
                let maxDaily = 0;
                for (const d of mdl.perDay) {
                  const v = metric() === "tokens" ? d.tokens : d.cost;
                  if (v > maxDaily) maxDaily = v;
                }

                return (
                  <div>
                    {/* Metrics grid */}
                    <div
                      style={{
                        display: "grid",
                        "grid-template-columns": "repeat(2, minmax(0, 1fr))",
                        gap: "4px 14px",
                        "margin-top": "10px",
                        "font-size": "13px",
                        "font-variant-numeric": "tabular-nums",
                      }}
                    >
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cost")}</span><span style={{ color: "var(--fig-cost)", "font-weight": 600 }}>{fmtCost(mdl.totals.cost)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.tokens")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(mdl.totals.tokens)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.input")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(mdl.totals.input)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.output")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(mdl.totals.output)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheRead")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtTokens(mdl.totals.cacheRead)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheWrite")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{mdl.totals.cacheWrite > 0 ? fmtTokens(mdl.totals.cacheWrite) : "—"}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.cacheHit")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{cacheHitPercent(mdl.totals.cacheRead, mdl.totals.tokens).toFixed(2)}%</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.messages")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{fmtNumber(mdl.totals.messages)}</span></div>
                      <div style={{ display: "flex" }}><span style={{ flex: 1, color: "var(--faint)", "font-weight": 400 }}>{t("usage.lastUsed")}</span><span style={{ color: "var(--fg)", "font-weight": 600 }}>{mdl.lastUsed || "—"}</span></div>
                    </div>

                    {/* Breakdown by Agent */}
                    <div style={{ "font-size": "12.5px", color: "var(--faint)", "margin-top": "12px" }}>
                      {t("usage.byAgent")}
                    </div>
                    <For each={mdl.agents}>
                      {(ag) => (
                        <div
                          style={{
                            "border-top": "1px solid var(--line)",
                            "margin-top": "8px",
                            "padding-top": "8px",
                            display: "flex",
                            "align-items": "baseline",
                            gap: "6px",
                            "font-variant-numeric": "tabular-nums",
                          }}
                        >
                          <span style={{ width: "9px", height: "9px", "border-radius": "50%", background: `var(--agent-${ag.agent})`, "align-self": "center" }} />
                          <span style={{ flex: 1, "min-width": 0, "font-weight": 600, "white-space": "nowrap", overflow: "hidden", "text-overflow": "ellipsis" }}>
                            {AGENT_NAMES[ag.agent] || ag.agent}
                          </span>
                          <span
                            style={{
                              "font-size": "13px",
                              color: "var(--fig-cost)",
                              "font-weight": 600,
                              "white-space": "nowrap",
                              flex: "none",
                              "text-align": "right",
                              "font-variant-numeric": "tabular-nums",
                            }}
                          >
                            {fmtCost(ag.cost)}
                          </span>
                          <span
                            style={{
                              "font-size": "13px",
                              color: "var(--fg)",
                              "font-weight": 600,
                              "white-space": "nowrap",
                              flex: "none",
                              width: "56px",
                              "text-align": "right",
                              "font-variant-numeric": "tabular-nums",
                            }}
                          >
                            {fmtTokens(ag.tokens)}
                          </span>
                        </div>
                      )}
                    </For>

                    {/* Sparkline (multi-day period only) */}
                    <Show when={!isSingleDay() && mdl.perDay.length > 1}>
                      <div style={{ "font-size": "12.5px", color: "var(--faint)", "margin-top": "12px" }}>
                        {t("usage.modelUsagePerDay")}
                      </div>
                      <div style={{ display: "flex", "align-items": "flex-end", gap: "2px", height: "44px", "margin-top": "6px" }}>
                        <For each={mdl.perDay}>
                          {(d) => {
                            const v = metric() === "tokens" ? d.tokens : d.cost;
                            const h = maxDaily > 0 ? `${Math.max(2, (v / maxDaily) * 44)}px` : "2px";
                            return (
                              <span
                                style={{
                                  flex: 1,
                                  "min-width": 0,
                                  height: h,
                                  "border-radius": "2px 2px 0 0",
                                  background: col,
                                }}
                              />
                            );
                          }}
                        </For>
                      </div>
                    </Show>
                  </div>
                );
              })()}
            </Show>
        </UsageBottomSheet>
      </Show>

      {/* Custom Range Sheet */}
      <Show when={sheet() === "custom"}>
        <UsageBottomSheet onClose={() => setSheet("none")}>
            <div style={{ display: "flex", "align-items": "center" }}>
              <div style={{ flex: 1 }}>
                <div style={{ "font-weight": 650, "font-size": "17px" }}>{t("usage.customTitle")}</div>
                <div style={{ "font-size": "12.5px", color: "var(--faint)", "margin-top": "2px" }}>
                  {t("usage.customHint", { from: usageReport()?.firstDay || "2026-01-01" })}
                </div>
              </div>
              <button class="cbtn" aria-label={t("common.close")} onClick={() => setSheet("none")}>
                <Icon name="x" />
              </button>
            </div>
            <div style={{ display: "grid", "grid-template-columns": "repeat(2, minmax(0, 1fr))", gap: "10px", "margin-top": "14px" }}>
              <label style={{ display: "flex", "flex-direction": "column", gap: "4px", "font-size": "12.5px", color: "var(--muted)" }}>
                {t("usage.start")}
                <input
                  type="date"
                  value={draftFrom()}
                  min={usageReport()?.firstDay || "2026-01-01"}
                  max={todayYMD()}
                  onChange={(e) => setDraftFrom(e.currentTarget.value)}
                  style={{
                    height: "44px",
                    "border-radius": "12px",
                    border: "1px solid var(--line2)",
                    background: "var(--bg)",
                    color: "var(--fg)",
                    font: "inherit",
                    "font-size": "15px",
                    padding: "0 10px",
                  }}
                />
              </label>
              <label style={{ display: "flex", "flex-direction": "column", gap: "4px", "font-size": "12.5px", color: "var(--muted)" }}>
                {t("usage.end")}
                <input
                  type="date"
                  value={draftTo()}
                  min={usageReport()?.firstDay || "2026-01-01"}
                  max={todayYMD()}
                  onChange={(e) => setDraftTo(e.currentTarget.value)}
                  style={{
                    height: "44px",
                    "border-radius": "12px",
                    border: "1px solid var(--line2)",
                    background: "var(--bg)",
                    color: "var(--fg)",
                    font: "inherit",
                    "font-size": "15px",
                    padding: "0 10px",
                  }}
                />
              </label>
            </div>
            <button
              onClick={() => {
                setCustomFrom(draftFrom());
                setCustomTo(draftTo());
                setRange("custom");
                setSheet("none");
              }}
              style={{
                "margin-top": "16px",
                width: "100%",
                height: "48px",
                "border-radius": "14px",
                border: 0,
                background: "var(--accent)",
                color: "var(--onaccent)",
                font: "inherit",
                "font-weight": 650,
              }}
            >
              {t("usage.apply")}
            </button>
        </UsageBottomSheet>
      </Show>

      {/* Agent Filter Sheet */}
      <Show when={sheet() === "filter"}>
        <UsageOverlay onClose={() => setSheet("none")}>
          <div
            style={{
              position: "absolute",
              left: "8px",
              right: "8px",
              top: "220px",
              background: "var(--surface)",
              border: "1px solid var(--line2)",
              "border-radius": "22px",
              padding: "16px 16px 12px",
            }}
          >
            <div style={{ display: "flex", "align-items": "center" }}>
              <span style={{ "font-weight": 650, "font-size": "16px", flex: 1 }}>{t("usage.filterTitle")}</span>
              <button
                onClick={() => setActiveAgents(new Set(enabledAgentKeys()))}
                style={{ height: "32px", padding: "0 10px", border: 0, background: "none", color: "var(--accent)", font: "inherit", "font-size": "14px" }}
              >
                {t("usage.selectAll")}
              </button>
              <button class="cbtn" aria-label={t("common.close")} onClick={() => setSheet("none")}>
                <Icon name="x" />
              </button>
            </div>
            <div style={{ "font-size": "12.5px", color: "var(--faint)", "margin-bottom": "6px" }}>
              {t("usage.filterHint")}
            </div>

            <For each={enabledAgentKeys()}>
              {(ag) => {
                const on = () => activeAgents().has(ag);
                const toggle = () => {
                  const s = new Set(activeAgents());
                  if (s.has(ag)) {
                    if (s.size > 1) s.delete(ag);
                  } else {
                    s.add(ag);
                  }
                  setActiveAgents(s);
                };

                return (
                  <button
                    onClick={toggle}
                    style={{
                      display: "flex",
                      "align-items": "center",
                      gap: "12px",
                      width: "100%",
                      "min-height": "48px",
                      background: "none",
                      border: 0,
                      "border-top": "1px solid var(--line)",
                      color: "var(--fg)",
                      font: "inherit",
                      "text-align": "left",
                    }}
                  >
                    <span
                      style={{
                        width: "22px",
                        height: "22px",
                        "border-radius": "7px",
                        border: on() ? "2px solid var(--accent)" : "2px solid var(--line2)",
                        background: on() ? "var(--accent)" : "none",
                        display: "grid",
                        "place-items": "center",
                        color: "var(--onaccent)",
                        flex: "none",
                      }}
                    >
                      <Show when={on()}>
                        <Icon name="check" sm />
                      </Show>
                    </span>
                    <span style={{ width: "9px", height: "9px", "border-radius": "50%", background: `var(--agent-${ag})`, flex: "none" }} />
                    <span style={{ flex: 1 }}>{AGENT_NAMES[ag]}</span>
                  </button>
                );
              }}
            </For>

            <button
              onClick={() => setSheet("none")}
              style={{
                "margin-top": "8px",
                width: "100%",
                height: "46px",
                "border-radius": "14px",
                border: 0,
                background: "var(--accent)",
                color: "var(--onaccent)",
                font: "inherit",
                "font-weight": 650,
              }}
            >
              {t("common.close")}
            </button>
          </div>
        </UsageOverlay>
      </Show>

      <Show when={state.usageEnabled}>
        <button
          class="tp-fab"
          classList={{ "nav-hidden": state.navHidden }}
          aria-label={t("app.reload")}
          onClick={() => void loadUsage(true)}
        >
          <span
            style={{
              display: "inline-flex",
              "align-items": "center",
              "justify-content": "center",
              animation: isLoading() ? "sp .8s linear infinite" : "none",
            }}
          >
            <Icon name="refresh" />
          </span>
        </button>
      </Show>
    </div>
  );
}
