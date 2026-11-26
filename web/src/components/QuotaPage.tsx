import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { createScrollNavHandler, setting, state } from "../store";
import { fetchQuota } from "../api";
import { QUOTA_PROVIDER_KEYS as ORDERED_PROVIDERS, scanIntervalToMs } from "../shared/defaults";
import { quotaLevel, quotaPace } from "../shared/quota";
import { formatMoney } from "../shared/money";
import { FeatureDisabledNotice, Icon } from "./ui";
import { t } from "../i18n";


const PROVIDER_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  antigravity: "Antigravity",
};

export function formatDurationShort(ms: number): string {
  const min = Math.max(0, Math.floor(ms / 60000));
  const D = 1440;
  if (min >= D) return `${Math.floor(min / D)}d ${Math.floor((min % D) / 60)}h`;
  if (min >= 60) return `${Math.floor(min / 60)}h ${min % 60}m`;
  return `${min}m`;
}

function windowStatusColor(usedPercent: number, pace: ReturnType<typeof quotaPace>): string {
  const l = quotaLevel(usedPercent);
  if (l === "exhausted" || pace?.kind === "ahead") return "var(--danger)";
  if (pace?.kind === "on_track" || l === "warn") return "var(--warn)";
  return "var(--addfg)";
}

function formatTime(ts: number): string {
  const d = new Date(ts);
  return `${String(d.getHours()).padStart(2, "0")}:${String(d.getMinutes()).padStart(2, "0")}:${String(d.getSeconds()).padStart(2, "0")}`;
}

export function QuotaPage() {
  const [refreshing, setRefreshing] = createSignal(false);
  const [now, setNow] = createSignal(Date.now());

  const isLoading = () => !state.configReady || state.quotaLoading || refreshing();

  const checkStaleRefresh = () => {
    if (!state.connected || !state.configReady || !state.quotaEnabled) return;
    if (typeof document !== "undefined" && document.visibilityState !== "visible") return;
    if (isLoading()) return;
    const staleMs = scanIntervalToMs(state.usageScanInterval) + 30_000;
    if (state.quotaUpdated === null || Date.now() - state.quotaUpdated > staleMs) {
      void fetchQuota();
    }
  };

  onMount(() => {
    if (state.quotaEnabled && state.configReady && state.quotaSnapshots.length === 0) {
      void fetchQuota();
    } else {
      checkStaleRefresh();
    }
    const onVisibility = () => {
      if (document.visibilityState === "visible") checkStaleRefresh();
    };
    if (typeof document !== "undefined") {
      document.addEventListener("visibilitychange", onVisibility);
    }
    const timer = setInterval(() => {
      setNow(Date.now());
      checkStaleRefresh();
    }, 30000);
    onCleanup(() => {
      clearInterval(timer);
      if (typeof document !== "undefined") {
        document.removeEventListener("visibilitychange", onVisibility);
      }
    });
  });

  createEffect(
    on(
      () => state.reconnectNonce,
      () => {
        if (state.configReady) void fetchQuota();
      },
      { defer: true },
    ),
  );

  const doRefresh = async () => {
    if (!state.quotaEnabled || refreshing()) return;
    setRefreshing(true);
    try {
      await fetchQuota(true);
    } finally {
      setRefreshing(false);
    }
  };


  const used = () => state.quotaDisplay === "used";

  const enabledProviderKeys = () => {
    const list: (typeof ORDERED_PROVIDERS)[number][] = [];
    for (const key of ORDERED_PROVIDERS) {
      if (state.quotaProviders[key] !== false) list.push(key);
    }
    return list.length > 0 ? list : (ORDERED_PROVIDERS as readonly (typeof ORDERED_PROVIDERS)[number][]);
  };

  const showEmptyProviders = () => setting("display.show_empty_providers", false);

  const enabledProviders = () => {
    const list: typeof state.quotaSnapshots = [];
    for (const key of ORDERED_PROVIDERS) {
      if (state.quotaProviders[key] === false) continue;
      const found = state.quotaSnapshots.find((s) => s.provider === key);
      if (found) {
        list.push(found);
      } else if (showEmptyProviders()) {
        list.push({ provider: key, enabled: true, snapshot: null, error: null });
      }
    }
    return list;
  };

  const hasAnyData = () => enabledProviders().some((p) => p.snapshot && p.snapshot.windows && p.snapshot.windows.length > 0);
  const showSkeleton = () => isLoading() && !hasAnyData();
  const onScroll = createScrollNavHandler();

  return (
    <div class="page">
      <div class="scroll" onScroll={onScroll}>
        <div style={{ padding: "calc(12px + env(safe-area-inset-top)) 16px calc(104px + env(safe-area-inset-bottom))" }}>
          <Show when={state.quotaEnabled} fallback={<FeatureDisabledNotice message={t("quota.disabledNotice")} />}>
          <div
            style={{
              display: "flex",
              "align-items": "center",
              "justify-content": "space-between",
              gap: "10px",
              padding: "2px 2px 8px",
              "font-size": "12px",
              color: "var(--muted)",
              "font-variant-numeric": "tabular-nums",
            }}
          >
            <span style={{ "font-size": "12px", "font-weight": 650, "letter-spacing": ".04em", "text-transform": "uppercase", color: "var(--faint)" }}>
              {t("home.quota")}
            </span>
            <Show when={isLoading() || state.quotaUpdated}>
              <span
                style={{
                  display: "inline-flex",
                  "align-items": "center",
                  gap: "5px",
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
                  ? (refreshing() ? t("quota.refreshing") : t("quota.loading"))
                  : t("quota.refreshedAt", { time: formatTime(state.quotaUpdated!) })}
              </span>
            </Show>
          </div>

          {/* Skeleton Template when loading without data */}
          <Show when={showSkeleton()}>
            <div
              style={{
                display: "flex",
                "align-items": "center",
                "justify-content": "center",
                gap: "8px",
                padding: "16px 0 6px",
                color: "var(--muted)",
                "font-size": "13px",
              }}
            >
              <div class="spin" style={{ width: "14px", height: "14px", "border-width": "2px" }} />
              <span>{t("quota.loading")}</span>
            </div>
            <div style={{ "margin-top": "6px", display: "flex", "flex-direction": "column", gap: "12px" }}>
              <For each={enabledProviderKeys()}>
                {(key) => (
                  <div
                    style={{
                      background: "var(--surface)",
                      border: "1px solid var(--line)",
                      "border-radius": "18px",
                      padding: "16px 16px 14px",
                      display: "flex",
                      "flex-direction": "column",
                      gap: "14px",
                    }}
                  >
                    <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
                      <div class="skel" style={{ width: "8px", height: "8px", "border-radius": "50%", flex: "none" }} />
                      <span
                        style={{
                          "font-weight": 700,
                          "font-size": "15px",
                          "letter-spacing": ".02em",
                          color: "var(--muted)",
                          opacity: 0.7,
                        }}
                      >
                        {PROVIDER_NAMES[key] || key.toUpperCase()}
                      </span>
                      <div style={{ flex: 1 }} />
                      <div class="skel" style={{ width: "64px", height: "14px", "border-radius": "10px" }} />
                    </div>
                    <div style={{ display: "flex", "flex-direction": "column", gap: "10px" }}>
                      <div style={{ display: "flex", "justify-content": "space-between", "align-items": "center" }}>
                        <div class="skel" style={{ width: "95px", height: "14px" }} />
                        <div class="skel" style={{ width: "55px", height: "14px" }} />
                      </div>
                      <div class="skel" style={{ width: "100%", height: "8px", "border-radius": "4px" }} />
                      <div style={{ display: "flex", "justify-content": "space-between", "align-items": "center" }}>
                        <div class="skel" style={{ width: "75px", height: "12px" }} />
                        <div class="skel" style={{ width: "85px", height: "12px" }} />
                      </div>
                    </div>
                  </div>
                )}
              </For>
            </div>
          </Show>

          {/* Empty state (only when loading finished and still no data) */}
          <Show when={!showSkeleton() && !hasAnyData() && !(showEmptyProviders() && enabledProviders().length > 0)}>
            <div
              style={{
                margin: "14px 0 0",
                background: "var(--surface)",
                border: "1px solid var(--line)",
                "border-radius": "18px",
                padding: "28px 20px",
                "text-align": "center",
                color: "var(--muted)",
                "font-size": "14px",
              }}
            >
              {t("quota.noData")}
            </div>
          </Show>

          {/* Provider cards */}
          <Show when={!showSkeleton()}>
            <div style={{ "margin-top": "10px", display: "flex", "flex-direction": "column", gap: "12px" }}>
            <For each={enabledProviders()}>
              {(p) => {
                const providerDotColor = () => {
                  const windows = p.snapshot?.windows ?? [];
                  const colors = windows.map((w) => windowStatusColor(w.usedPercent, quotaPace(w, now())));
                  if (colors.includes("var(--danger)")) return "var(--danger)";
                  if (colors.includes("var(--warn)")) return "var(--warn)";
                  return "var(--addfg)";
                };

                return (
                  <div>
                    {/* Error banner above card if failed */}
                    <Show when={p.error}>
                      <div
                        style={{
                          background: "var(--dangerbg)",
                          border: "1px solid var(--danger)",
                          "border-radius": "14px",
                          padding: "10px 14px",
                          "font-size": "13px",
                          color: "var(--danger)",
                          display: "flex",
                          gap: "8px",
                          "align-items": "center",
                          "margin-bottom": "8px",
                        }}
                      >
                        <Icon name="alert" sm />
                        <span>{t("quota.errorPrefix", { error: p.error!.message })}</span>
                      </div>
                    </Show>

                    {/* Card */}
                    <Show when={p.snapshot || showEmptyProviders()}>
                      <div class="tp-card" style={{ padding: "16px 18px 10px" }}>
                        <div style={{ display: "flex", "align-items": "center", gap: "9px" }}>
                          <span
                            style={{
                              width: "9px",
                              height: "9px",
                              "border-radius": "50%",
                              background: providerDotColor(),
                              "box-shadow": `0 0 0 3px color-mix(in srgb, ${providerDotColor()} 20%, transparent)`,
                              flex: "none",
                            }}
                          />
                          <span
                            style={{
                              "font-weight": 720,
                              "font-size": "16px",
                              "letter-spacing": "-.01em",
                              color: "var(--fg)",
                            }}
                          >
                            {PROVIDER_NAMES[p.provider] || p.provider}
                          </span>
                          <Show when={state.accountDisplay !== "none" && p.snapshot?.plan}>
                            <span
                              class="tp-pill"
                              style={{
                                background: "var(--accentbg)",
                                color: "var(--accent)",
                                "font-size": "11px",
                              }}
                            >
                              {p.snapshot?.plan}
                            </span>
                          </Show>
                        </div>
                        <Show when={state.accountDisplay === "full" && p.snapshot?.account}>
                          <div style={{ "font-size": "12px", color: "var(--muted)", "margin-top": "3px", "padding-left": "18px" }}>
                            {p.snapshot?.account}
                          </div>
                        </Show>

                        <Show when={!p.snapshot || (p.snapshot.windows?.length ?? 0) === 0}>
                          <div style={{ padding: "12px 0 6px", color: "var(--faint)", "font-size": "12.5px" }}>
                            {t("quota.emptyProvider")}
                          </div>
                        </Show>

                        {/* Windows */}
                        <For each={p.snapshot?.windows ?? []}>
                          {(w) => {
                            const u = () => w.usedPercent;
                            const lvl = () => quotaLevel(u());
                            const pResult = () => quotaPace(w, now());

                            const displayWindowLabel = (label: string) => {
                              const lower = label.trim().toLowerCase();
                              if (lower === "session (5h)" || lower === "5h" || lower === "5 hours") return "5h";
                              if (lower === "weekly (7d)" || lower === "weekly") return "Weekly";
                              return label.replace(/^Session \(5h\)/i, "5h").replace(/^Weekly \(7d\)/i, "Weekly");
                            };

                            const tag = () => {
                              const base = displayWindowLabel(w.label);
                              if (!w.family) return base;
                              if (base.toLowerCase().includes(w.family.toLowerCase())) return base;
                              return `${base} · ${w.family}`;
                            };

                            const resetText = () => {
                              if (u() <= 0) return t("quota.notStarted");
                              if (!w.resetsAt) return "";
                              const diff = w.resetsAt - now();
                              return t("quota.resetIn", { time: formatDurationShort(diff) });
                            };

                            const barWidth = () => {
                              const share = used() ? u() : Math.max(0, 100 - u());
                              return `${Math.min(100, Math.max(0, share))}%`;
                            };

                            const statusColor = () => windowStatusColor(u(), pResult());

                            const barColor = () => statusColor();

                            const markerPos = () => {
                              const pace = pResult();
                              if (!pace) return null;
                              const exp = used() ? pace.expectedPercent : 100 - pace.expectedPercent;
                              return Math.min(100, Math.max(0, exp));
                            };

                            const firstText = () =>
                              used()
                                ? t("quota.used", { pct: Math.round(u()).toString() })
                                : t("quota.remaining", { pct: Math.round(Math.max(0, 100 - u())).toString() });

                            const firstColor = () => {
                              if (statusColor() === "var(--danger)") return "var(--danger)";
                              if (statusColor() === "var(--warn)") return "var(--warn)";
                              return "var(--fg)";
                            };

                            const paceInfo = () => {
                              if (u() >= 100) return { text: t("quota.exhausted"), color: "var(--danger)" };
                              const pace = pResult();
                              if (!pace) return null;
                              if (pace.kind === "on_track") return { text: t("quota.onTrack"), color: "var(--warn)" };
                              if (pace.kind === "ahead") {
                                const eta = pace.etaMs ? formatDurationShort(pace.etaMs) : "";
                                return {
                                  text: t("quota.ahead", { deficit: Math.round(pace.deficit).toString(), eta }),
                                  color: "var(--danger)",
                                };
                              }
                              return {
                                text: t("quota.behind", { deficit: Math.round(-pace.deficit).toString() }),
                                color: "var(--addfg)",
                              };
                            };

                            const bigPct = () => `${Math.round(used() ? u() : Math.max(0, 100 - u()))}%`;

                            return (
                              <div style={{ padding: "12px 0 8px", "border-top": "1px solid var(--line)", "margin-top": "10px" }}>
                                <div style={{ display: "flex", "align-items": "baseline", gap: "8px" }}>
                                  <span style={{ "font-size": "13.5px", "font-weight": 650, flex: 1 }}>
                                    {tag()}
                                  </span>
                                  <span
                                    style={{
                                      "font-size": "19px",
                                      "font-weight": 750,
                                      "letter-spacing": "-.02em",
                                      color: firstColor(),
                                      "font-variant-numeric": "tabular-nums",
                                    }}
                                  >
                                    {bigPct()}
                                  </span>
                                </div>
                                <div style={{ position: "relative", height: "16px", margin: "6px 0", display: "flex", "align-items": "center" }}>
                                  <div
                                    style={{
                                      width: "100%",
                                      height: "10px",
                                      "border-radius": "999px",
                                      background: "var(--line2)",
                                      overflow: "hidden",
                                    }}
                                  >
                                    <div
                                      style={{
                                        height: "10px",
                                        "border-radius": "999px",
                                        width: barWidth(),
                                        background: barColor(),
                                        transition: "width .25s cubic-bezier(.2,.8,.2,1)",
                                      }}
                                    />
                                  </div>
                                  <Show when={markerPos() !== null}>
                                    <span
                                      style={{
                                        position: "absolute",
                                        top: "1px",
                                        left: `calc(${markerPos()!.toFixed(1)}% - 1.5px)`,
                                        width: "3px",
                                        height: "14px",
                                        "border-radius": "999px",
                                        background: "var(--fg)",
                                        "box-shadow": "0 0 0 1.5px var(--surface)",
                                      }}
                                    />
                                  </Show>
                                </div>
                                <div style={{ display: "flex", "align-items": "center", gap: "8px", "font-size": "12px", "font-variant-numeric": "tabular-nums" }}>
                                  <span style={{ color: "var(--muted)" }}>{firstText()} · {resetText()}</span>
                                  <span style={{ flex: 1 }} />
                                  <Show when={paceInfo()}>
                                    <span
                                      class="tp-pill"
                                      style={{
                                        color: paceInfo()!.color,
                                        background: `color-mix(in srgb, ${paceInfo()!.color} 14%, transparent)`,
                                        "font-size": "11px",
                                      }}
                                    >
                                      {paceInfo()!.text}
                                    </span>
                                  </Show>
                                </div>
                              </div>
                            );
                          }}
                        </For>

                        {/* Extra Usage (Claude) */}
                        <Show when={p.snapshot?.extraUsage}>
                          {(eu) => {
                            const fmtAmount = (amount: number, currency: string) => formatMoney(amount, currency);
                            const cur = () => eu().currency ?? "USD";
                            const pct = () => (eu().limit && eu().limit! > 0 ? Math.min(100, Math.max(0, (eu().used / eu().limit!) * 100)) : null);
                            return (
                              <div
                                style={{
                                  "border-top": "1px solid var(--line)",
                                  "margin-top": "8px",
                                  padding: "10px 0 6px",
                                  "font-size": "12.5px",
                                  color: "var(--muted)",
                                  "font-variant-numeric": "tabular-nums",
                                }}
                              >
                                <div style={{ display: "flex", "justify-content": "space-between", "align-items": "center" }}>
                                  <span>
                                    {t("quota.extraUsage", {
                                      used: fmtAmount(eu().used, cur()),
                                      limit: eu().limit !== null ? fmtAmount(eu().limit!, cur()) : "—",
                                    })}
                                  </span>
                                  <Show when={pct() !== null}>
                                    <b style={{ color: "var(--fg)", "font-weight": 600 }}>{Math.round(pct()!)}%</b>
                                  </Show>
                                </div>
                                <Show when={pct() !== null}>
                                  <div style={{ height: "6px", "border-radius": "999px", background: "var(--line2)", overflow: "hidden", "margin-top": "6px" }}>
                                    <div style={{ height: "6px", "border-radius": "999px", width: `${pct()!}%`, background: "var(--accent)" }} />
                                  </div>
                                </Show>
                              </div>
                            );
                          }}
                        </Show>

                        {/* Reset Credits */}
                        <Show when={p.snapshot?.credits && p.snapshot.credits.length > 0}>
                          <div
                            style={{
                              "border-top": "1px solid var(--line)",
                              "margin-top": "8px",
                              padding: "10px 0 6px",
                              "font-size": "12.5px",
                            }}
                          >
                            <div style={{ display: "flex", color: "var(--fg)", "font-weight": 600 }}>
                              <span style={{ flex: 1 }}>{t("quota.creditsTitle")}</span>
                              <span>{t("quota.creditsExpiry")}</span>
                            </div>
                            <For each={p.snapshot?.credits}>
                              {(c) => (
                                <div style={{ display: "flex", color: "var(--muted)", "margin-top": "4px", "font-variant-numeric": "tabular-nums" }}>
                                  <span style={{ flex: 1 }}>{c.label ?? (c.amount != null ? c.amount.toString() : "—")}</span>
                                  <span>{c.expiresAt ? new Date(c.expiresAt).toLocaleDateString() : "—"}</span>
                                </div>
                              )}
                            </For>
                          </div>
                        </Show>
                      </div>
                    </Show>
                  </div>
                );
              }}
            </For>
          </div>
          </Show>
          </Show>
        </div>
      </div>

      <Show when={state.quotaEnabled}>
        <button
          class="tp-fab"
          classList={{ "nav-hidden": state.navHidden }}
          aria-label={t("app.reload")}
          onClick={doRefresh}
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
