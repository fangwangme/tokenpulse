import { createEffect, createSignal, For, on, onMount, Show } from "solid-js";
import { loadKeeper, patchSettings, pingKeeper } from "../api";
import { createScrollNavHandler, showToast, state, type KeeperAgent, type KeeperPayload } from "../store";
import { AGENT_NAMES } from "../shared/defaults";
import { Icon } from "./ui";
import { t } from "../i18n";

function formatTime(value: string | null | undefined): string {
  if (!value) return "—";
  const date = new Date(value);
  if (Number.isNaN(date.getTime())) return "—";
  return date.toLocaleString(undefined, {
    month: "short",
    day: "numeric",
    hour: "2-digit",
    minute: "2-digit",
  });
}

function agentName(id: string): string {
  return AGENT_NAMES[id] || id;
}

export function KeeperPage() {
  const [keeper, setKeeper] = createSignal<KeeperPayload | null>(null);
  const [error, setError] = createSignal("");
  const [busy, setBusy] = createSignal("");
  const [wakeups, setWakeups] = createSignal<Record<string, string>>({});
  const [expanded, setExpanded] = createSignal<Record<string, boolean>>({});

  const load = async () => {
    setError("");
    try {
      const value = await loadKeeper();
      setKeeper(value);
      const times: Record<string, string> = {};
      for (const [id, agent] of Object.entries(value.agents || {})) {
        times[id] = agent.dailyWakeupTime || "10:30";
      }
      setWakeups(times);
    } catch (err: any) {
      setError(err?.message || t("usage.loadFailed"));
    }
  };

  onMount(() => void load());
  createEffect(
    on(
      () => state.keeperNonce,
      () => void load(),
      { defer: true },
    ),
  );

  const update = async (key: string, value: unknown) => {
    try {
      await patchSettings({ [key]: value });
      await load();
    } catch (err: any) {
      showToast(err?.message || t("usage.loadFailed"));
    }
  };

  const ping = async (agent: string, group?: string) => {
    const key = agent + (group || "");
    setBusy(key);
    try {
      await pingKeeper(agent, group);
      showToast(t("quota.refreshing"));
      window.setTimeout(() => void load(), 700);
    } catch (err: any) {
      showToast(err?.message || t("usage.loadFailed"));
    } finally {
      setBusy("");
    }
  };

  const agentCard = (id: string, agent: KeeperAgent) => (
    <div class="tp-card" style={{ "margin-top": "10px", overflow: "hidden" }}>
      <div class="li" style={{ "align-items": "center" }}>
        <span style={{ width: "9px", height: "9px", "border-radius": "50%", background: `var(--agent-${id === "agy" ? "agy" : id})`, flex: "none" }} />
        <div class="tx">
          {agentName(id)}
          <Show when={agent.running}>
            <small>{t("keeper.running")}</small>
          </Show>
        </div>
        <Show when={agent.running}>
          <span class="sdot working" />
        </Show>
      </div>
      <button class="li" onClick={() => void update(`keeper.agents.${id}.session_keeper_enabled`, !agent.sessionKeeperEnabled)}>
        <div class="tx">{t("keeper.session")}</div>
        <span class={`tg${agent.sessionKeeperEnabled ? "" : " off"}`} />
      </button>
      <button class="li" onClick={() => void update(`keeper.agents.${id}.weekly_keeper_enabled`, !agent.weeklyKeeperEnabled)}>
        <div class="tx">{t("keeper.weekly")}</div>
        <span class={`tg${agent.weeklyKeeperEnabled ? "" : " off"}`} />
      </button>
      <Show when={id === "agy"}>
        <button class="li" onClick={() => void update("keeper.agents.agy.weekly_all_groups", !agent.weeklyAllGroups)}>
          <div class="tx">{t("keeper.allGroups")}</div>
          <span class={`tg${agent.weeklyAllGroups ? "" : " off"}`} />
        </button>
      </Show>
      <div class="li">
        <div class="tx">{t("keeper.wakeup")}</div>
        <input
          class="inp"
          type="time"
          style={{ width: "140px", height: "36px", "font-size": "13px" }}
          value={wakeups()[id] || agent.dailyWakeupTime || "10:30"}
          onInput={(event) => setWakeups({ ...wakeups(), [id]: event.currentTarget.value })}
          onChange={(event) => void update(`keeper.agents.${id}.daily_wakeup_time`, event.currentTarget.value)}
        />
      </div>
      <div class="li" style={{ "flex-wrap": "wrap", gap: "8px" }}>
        <button
          class="btn acc"
          style={{ "min-height": "34px", padding: "0 14px", "font-size": "13px", "border-radius": "17px" }}
          disabled={busy() === id}
          onClick={() => void ping(id)}
        >
          {busy() === id ? t("keeper.running") : t("keeper.ping")}
        </button>
        <For each={agent.targets || []}>
          {(target) => (
            <Show when={target.group}>
              <button
                class="btn"
                style={{ "min-height": "34px", padding: "0 14px", "font-size": "13px", "border-radius": "17px" }}
                disabled={busy() === id + target.group}
                onClick={() => void ping(id, target.group)}
              >
                {target.group} · {t("keeper.ping")}
              </button>
            </Show>
          )}
        </For>
      </div>
    </div>
  );

  const onScroll = createScrollNavHandler();

  return (
    <div class="page">
      <div class="scroll" onScroll={onScroll}>
        <div class="settings">
          <Show when={error()}>
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
                "margin-bottom": "10px",
              }}
            >
              <Icon name="alert" sm />
              <span style={{ flex: 1 }}>{error()}</span>
              <button class="lnk" onClick={() => void load()}>{t("usage.retry")}</button>
            </div>
          </Show>
          <Show when={keeper()}>
            <div
              class={keeper()!.engine ? "tp-hero" : "tp-card"}
              style={{
                background: keeper()!.engine
                  ? "radial-gradient(100% 100% at 0% 0%, color-mix(in srgb, var(--addfg) 14%, transparent), transparent 65%), var(--surface)"
                  : "var(--surface)",
              }}
            >
              <button class="li" onClick={() => void update("keeper.enabled", !keeper()!.engine)}>
                <div class="tx">
                  <span style={{ "font-weight": 680 }}>{t("keeper.engine")}</span>
                  <small>{keeper()!.engine ? t("settings.on") : t("settings.off")}</small>
                </div>
                <span class={`tg${keeper()!.engine ? "" : " off"}`} />
              </button>
            </div>
            <Show when={!keeper()!.engine}>
              <p class="note">{t("keeper.engineOff")}</p>
            </Show>
            <For each={Object.entries(keeper()!.agents || {})}>{([id, agent]) => agentCard(id, agent)}</For>
            <div class="sec">{t("keeper.recent")}</div>
            <div class="tp-card" style={{ overflow: "hidden" }}>
              <Show
                when={(keeper()!.executions || []).length > 0}
                fallback={<div class="li"><div class="tx">{t("keeper.none")}</div></div>}
              >
                <For each={[...keeper()!.executions].reverse()}>
                  {(execution, idx) => {
                    const rowKey = () => `${execution.time}-${execution.agent}-${idx()}`;
                    const isOpen = () => Boolean(expanded()[rowKey()]);
                    const cleanReply = () => (execution.reply || "").replace(/\s+/g, " ").trim();
                    const ok = () => execution.status === "success";
                    const triggerLabel = () => {
                      const raw = (execution.trigger || "").toLowerCase();
                      if (raw === "session" || raw === "daily" || raw === "5h") return "5h";
                      if (raw === "weekly") return "weekly";
                      return execution.trigger;
                    };
                    return (
                      <button
                        class="li tp-press"
                        style={{ "align-items": "flex-start", "text-align": "left" }}
                        onClick={() => setExpanded({ ...expanded(), [rowKey()]: !isOpen() })}
                      >
                        <span
                          style={{
                            width: "8px",
                            height: "8px",
                            "border-radius": "50%",
                            background: ok() ? "var(--addfg)" : "var(--danger)",
                            "margin-top": "6px",
                            flex: "none",
                          }}
                        />
                        <div class="tx" style={{ "min-width": 0 }}>
                          <div style={{ display: "flex", "align-items": "center", gap: "6px", "flex-wrap": "wrap" }}>
                            <span style={{ "font-weight": 650 }}>{agentName(execution.agent)}</span>
                            <span class="tp-pill" style={{ background: "var(--code)", color: "var(--muted)", "font-size": "10.5px" }}>
                              {triggerLabel()}
                            </span>
                            <Show when={execution.model}>
                              <span style={{ "font-size": "11.5px", color: "var(--faint)" }}>{execution.model}</span>
                            </Show>
                          </div>
                          <small>{formatTime(execution.time)}</small>
                          <Show when={cleanReply()}>
                            <small
                              style={{
                                "margin-top": "4px",
                                color: "var(--muted)",
                                display: isOpen() ? "block" : "-webkit-box",
                                "-webkit-line-clamp": isOpen() ? "unset" : "2",
                                "-webkit-box-orient": "vertical",
                                overflow: "hidden",
                                "overflow-wrap": "anywhere",
                              }}
                            >
                              {cleanReply()}
                            </small>
                          </Show>
                        </div>
                        <span
                          class="tp-pill"
                          style={{
                            color: ok() ? "var(--addfg)" : "var(--danger)",
                            background: ok() ? "var(--addbg)" : "var(--dangerbg)",
                            "font-size": "11px",
                            flex: "none",
                          }}
                        >
                          {ok() ? t("keeper.success") : t("keeper.failed")}
                        </span>
                      </button>
                    );
                  }}
                </For>
              </Show>
            </div>
          </Show>
        </div>
      </div>
    </div>
  );
}
