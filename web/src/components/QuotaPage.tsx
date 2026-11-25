import { createEffect, createSignal, For, on, onCleanup, onMount, Show } from "solid-js";
import { navigate, state } from "../store";
import { fetchQuota } from "../api";
import { QUOTA_PROVIDER_KEYS as ORDERED_PROVIDERS, scanIntervalToMs } from "../shared/defaults";
import { quotaLevel, quotaPace } from "../shared/quota";
import { formatMoney } from "../shared/money";
import { FeatureDisabledNotice, Icon } from "./ui";
import { t } from "../i18n";


const PROVIDER_NAMES: Record<string, string> = {
  claude: "CLAUDE CODE",
  codex: "CODEX",
  antigravity: "ANTIGRAVITY",
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

  const enabledProviders = () => {
    const list: typeof state.quotaSnapshots = [];
    for (const key of ORDERED_PROVIDERS) {
      if (state.quotaProviders[key] === false) continue;
      const found = state.quotaSnapshots.find((s) => s.provider === key);
      if (found) list.push(found);
    }
    return list;
  };

  const hasAnyData = () => enabledProviders().some((p) => p.snapshot && p.snapshot.windows && p.snapshot.windows.length > 0);
  const showSkeleton = () => isLoading() && !hasAnyData();

  return (
    <div class="page">
      {/* Shared Header */}
      <div class="hdr">
        <div style={{ width: "44px" }} />
        <div class="ht">
          <div class="t">{t("quota.title")}</div>
          <div class="s">
            {state.quotaEnabled
              ? (isLoading()
                ? (refreshing() ? t("quota.refreshing") : t("quota.loading"))
                : state.quotaUpdated
                ? t("quota.refreshedAt", { time: formatTime(state.quotaUpdated) })
                : "")
              : ""}
          </div>
        </div>
        <Show when={state.quotaEnabled} fallback={<div style={{ width: "38px" }} />}>
          <button class="cbtn" aria-label={t("app.reload")} onClick={doRefresh}>
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

      <div class="scroll">
        <div style={{ padding: "0 16px calc(104px + env(safe-area-inset-bottom))" }}>
          <Show when={state.quotaEnabled} fallback={<FeatureDisabledNotice message={t("quota.disabledNotice")} />}>

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
          <Show when={!showSkeleton() && !hasAnyData()}>
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
              <div style={{ "margin-top": "12px" }}>
                <button
                  onClick={() => navigate({ name: "settings" })}
                  style={{ color: "var(--accent)", "font-weight": 600 }}
                >
                  {t("quota.manageProviders")}
                </button>
              </div>
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
                    <Show when={p.snapshot}>
                      <div
                        style={{
                          background: "var(--surface)",
                          border: "1px solid var(--line)",
                          "border-radius": "18px",
                          padding: "14px 16px 8px",
                        }}
                      >
                        <div style={{ display: "flex", "align-items": "center", gap: "8px" }}>
                          <span
                            style={{
                              width: "8px",
                              height: "8px",
                              "border-radius": "50%",
                              background: providerDotColor(),
                              flex: "none",
                            }}
                          />
                          <span
                            style={{
                              "font-weight": 700,
                              "font-size": "15px",
                              "letter-spacing": ".02em",
                              color: "var(--fg)",
                            }}
                          >
                            {PROVIDER_NAMES[p.provider] || p.provider.toUpperCase()}
                          </span>
                        </div>
                        <Show when={state.accountDisplay !== "none" && (p.snapshot?.plan || p.snapshot?.account)}>
                          <div style={{ "font-size": "12.5px", color: "var(--muted)", "margin-top": "2px" }}>
                            <Show when={p.snapshot?.plan}>
                              <b style={{ color: "var(--fg)", "font-weight": 600 }}>{p.snapshot?.plan}</b>
                            </Show>
                            <Show when={state.accountDisplay === "full" && p.snapshot?.account}>
                              <span>{p.snapshot?.plan ? " · " : ""}{p.snapshot?.account}</span>
                            </Show>
                          </div>
                        </Show>

                        {/* Windows */}
                        <For each={p.snapshot?.windows ?? []}>
                          {(w) => {
                            const u = () => w.usedPercent;
                            const lvl = () => quotaLevel(u());
                            const pResult = () => quotaPace(w, now());

                            const tag = () => {
                              if (!w.family) return w.label;
                              if (w.label.toLowerCase().includes(w.family.toLowerCase())) return w.label;
                              return `${w.label} · ${w.family}`;
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

                            return (
                              <div style={{ padding: "10px 0 8px", "border-top": "1px solid var(--line)", "margin-top": "8px" }}>
                                <div style={{ display: "flex", "align-items": "baseline", gap: "8px" }}>
                                  <span style={{ "font-size": "13.5px", "font-weight": 650, flex: 1, "letter-spacing": ".01em" }}>
                                    {tag()}
                                  </span>
                                  <span style={{ "font-size": "12.5px", color: "var(--muted)", "font-variant-numeric": "tabular-nums" }}>
                                    {resetText()}
                                  </span>
                                </div>
                                <div style={{ position: "relative", height: "14px", margin: "5px 0", display: "flex", "align-items": "center" }}>
                                  <div
                                    style={{
                                      width: "100%",
                                      height: "8px",
                                      "border-radius": "4px",
                                      background: "var(--line2)",
                                      overflow: "hidden",
                                    }}
                                  >
                                    <div
                                      style={{
                                        height: "8px",
                                        width: barWidth(),
                                        background: barColor(),
                                        transition: "width .2s ease-out",
                                      }}
                                    />
                                  </div>
                                  <Show when={markerPos() !== null}>
                                    <span
                                      style={{
                                        position: "absolute",
                                        top: 0,
                                        left: `calc(${markerPos()!.toFixed(1)}% - 1px)`,
                                        width: "2px",
                                        height: "14px",
                                        "border-radius": "1px",
                                        background: "var(--fg)",
                                        "box-shadow": "0 0 0 1px var(--surface)",
                                      }}
                                    />
                                  </Show>
                                </div>
                                <div style={{ display: "flex", "align-items": "baseline", gap: "10px", "font-size": "12.5px", "font-variant-numeric": "tabular-nums" }}>
                                  <span style={{ color: firstColor(), "font-weight": 600 }}>{firstText()}</span>
                                  <span style={{ flex: 1 }} />
                                  <Show when={paceInfo()}>
                                    <span style={{ color: paceInfo()!.color, "font-weight": 550, "text-align": "right" }}>
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
                            return (
                              <div
                                style={{
                                  "border-top": "1px solid var(--line)",
                                  "margin-top": "8px",
                                  padding: "10px 0 6px",
                                  "font-size": "12.5px",
                                  display: "flex",
                                  "justify-content": "space-between",
                                  color: "var(--muted)",
                                  "font-variant-numeric": "tabular-nums",
                                }}
                              >
                                <span>
                                  {t("quota.extraUsage", {
                                    used: fmtAmount(eu().used, cur()),
                                    limit: eu().limit !== null ? fmtAmount(eu().limit!, cur()) : "—",
                                  })}
                                </span>
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

          {/* Manage queried providers button (only when data is loaded) */}
          <Show when={!showSkeleton() && hasAnyData()}>
            <div style={{ "margin-top": "14px" }}>
              <button
                onClick={() => navigate({ name: "settings" })}
                style={{
                  display: "flex",
                  "align-items": "center",
                  "justify-content": "center",
                  gap: "6px",
                  width: "100%",
                  "min-height": "44px",
                  "border-radius": "12px",
                  border: "1px solid var(--line2)",
                  color: "var(--accent)",
                  "font-size": "14px",
                  "font-weight": 550,
                  background: "none",
                }}
              >
                <Icon name="sliders" sm />
                {t("quota.manageProviders")}
              </button>
            </div>
          </Show>
          </Show>
        </div>
      </div>
    </div>
  );
}
