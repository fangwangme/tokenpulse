import { createSignal, For, onMount, Show } from "solid-js";
import { patchSettings, getPushKey, addPushSubscription, removePushSubscription } from "../api";
import { createScrollNavHandler, saveLocale, saveTheme, setting, showToast, state } from "../store";
import { QUOTA_PROVIDER_KEYS } from "../shared/defaults";
import { LOCALES, setLocale, t, type LocalePref } from "../i18n";
import type { ThemeMode } from "../types";

const MODES: Array<[ThemeMode, "settings.followSystem" | "settings.light" | "settings.dark"]> = [
  ["system", "settings.followSystem"],
  ["light", "settings.light"],
  ["dark", "settings.dark"],
];

const SCAN_SECONDS = [60, 120, 300, 600, 900];
const LEVELS = ["off", "in_app", "terminal", "system"] as const;
const SOUNDS = ["chime", "Hero", "Glass", "Submarine", "none"] as const;

function levelLabel(level: (typeof LEVELS)[number]): string {
  switch (level) {
    case "off":
      return t("settings.notifOff");
    case "in_app":
      return t("settings.notifInApp");
    case "terminal":
      return t("settings.notifTerminal");
    case "system":
      return t("settings.notifSystem");
  }
}

function soundLabel(sound: (typeof SOUNDS)[number]): string {
  if (sound === "chime") return t("settings.soundChime");
  if (sound === "none") return t("settings.soundNone");
  return sound;
}

function urlBase64ToUint8Array(base64String: string): Uint8Array {
  const padding = "=".repeat((4 - (base64String.length % 4)) % 4);
  const base64 = (base64String + padding).replace(/-/g, "+").replace(/_/g, "/");
  const raw = atob(base64);
  const output = new Uint8Array(raw.length);
  for (let i = 0; i < raw.length; i += 1) output[i] = raw.charCodeAt(i);
  return output;
}

export function SettingsPage() {
  const [push, setPush] = createSignal<"unsupported" | "loading" | "on" | "off">("loading");
  const [pushBusy, setPushBusy] = createSignal(false);
  const [standalone, setStandalone] = createSignal(false);

  const update = async (key: string, value: unknown) => {
    try {
      await patchSettings({ [key]: value });
    } catch (error: any) {
      showToast(error?.message || t("usage.loadFailed"));
    }
  };

  const value = <T,>(key: string, fallback: T): T => setting<T>(key, fallback);
  const scanSeconds = () => {
    const seconds = Number(value("display.auto_refresh_secs", 300));
    return seconds > 0 ? seconds : 300;
  };

  onMount(() => {
    setStandalone(document.documentElement.dataset.mode === "standalone");
    if (!("serviceWorker" in navigator) || !("PushManager" in window) || !("Notification" in window)) {
      setPush("unsupported");
      return;
    }
    void navigator.serviceWorker
      .register("/sw.js")
      .then((registration) => registration.pushManager.getSubscription())
      .then((subscription) => setPush(subscription ? "on" : "off"))
      .catch(() => setPush("off"));
  });

  const togglePush = async () => {
    if (push() === "unsupported" || pushBusy()) return;
    setPushBusy(true);
    try {
      const registration = await navigator.serviceWorker.ready;
      if (push() === "on") {
        const subscription = await registration.pushManager.getSubscription();
        if (subscription) {
          await removePushSubscription(subscription.endpoint).catch(() => {});
          await subscription.unsubscribe();
        }
        setPush("off");
        return;
      }
      const permission = await Notification.requestPermission();
      if (permission !== "granted") {
        setPush("off");
        return;
      }
      const { publicKey } = await getPushKey();
      const subscription = await registration.pushManager.subscribe({
        userVisibleOnly: true,
        applicationServerKey: urlBase64ToUint8Array(publicKey) as BufferSource,
      });
      await addPushSubscription(subscription.toJSON());
      setPush("on");
    } catch (error: any) {
      showToast(error?.message || t("settings.pushError"));
    } finally {
      setPushBusy(false);
    }
  };

  const onScroll = createScrollNavHandler();

  return (
    <div class="page">
      <div class="scroll" onScroll={onScroll}>
        <div class="settings" style={{ "padding-top": "calc(10px + env(safe-area-inset-top))" }}>
          <div class="sec">{t("settings.appearance")}</div>
          <div class="tp-card" style={{ overflow: "hidden" }}>
            <div class="li" style={{ "flex-direction": "column", "align-items": "stretch", gap: "8px" }}>
              <div class="tx">{t("settings.appearance")}</div>
              <div class="seg">
                <For each={MODES}>
                  {([mode, label]) => (
                    <button class={state.themeMode === mode ? "on" : ""} onClick={() => saveTheme(mode)}>
                      {t(label)}
                    </button>
                  )}
                </For>
              </div>
            </div>
            <div class="li" style={{ "flex-direction": "column", "align-items": "stretch", gap: "8px" }}>
              <div class="tx">{t("settings.language")}</div>
              <div class="seg">
                <For
                  each={
                    [
                      ["system", t("settings.followSystem")],
                      ...Object.entries(LOCALES).map(([code, entry]) => [code, entry.label]),
                    ] as Array<[LocalePref, string]>
                  }
                >
                  {([code, label]) => (
                    <button
                      class={state.localePref === code ? "on" : ""}
                      onClick={() => {
                        saveLocale(code);
                        const languages = code === "system" ? navigator.languages : [code];
                        const resolved =
                          code === "system"
                            ? languages.some((tag) => tag.toLowerCase().startsWith("zh"))
                              ? "zh"
                              : "en"
                            : code;
                        setLocale(resolved);
                      }}
                    >
                      {label}
                    </button>
                  )}
                </For>
              </div>
            </div>
          </div>
          <p class="note">{t("settings.appearanceNote")}</p>

          <div class="sec">{t("settings.quotaUsage")}</div>
          <div class="tp-card" style={{ overflow: "hidden" }}>
            <div class="li" style={{ "flex-direction": "column", "align-items": "stretch", gap: "8px" }}>
              <div class="tx">{t("settings.usageScanInterval")}</div>
              <div class="seg" style={{ display: "grid", "grid-template-columns": "repeat(5, minmax(0, 1fr))", padding: "2px" }}>
                <For each={SCAN_SECONDS}>
                  {(seconds) => (
                    <button
                      class={scanSeconds() === seconds ? "on" : ""}
                      style={{ height: "30px", "font-size": "12px", padding: "0 2px" }}
                      onClick={() => void update("display.auto_refresh_secs", seconds)}
                    >
                      {`${seconds / 60}m`}
                    </button>
                  )}
                </For>
              </div>
            </div>
            <button class="li" onClick={() => void update("display.refresh_quota", !value("display.refresh_quota", true))}>
              <div class="tx">{t("settings.quotaEnabled")}</div>
              <span class={`tg${value("display.refresh_quota", true) ? "" : " off"}`} />
            </button>
            <div class="li">
              <div class="tx">{t("settings.quotaDisplay")}</div>
              <div
                class="seg"
                style={{
                  display: "grid",
                  "grid-template-columns": "repeat(2, minmax(0, 1fr))",
                  width: "168px",
                  padding: "2px",
                  "border-radius": "9px",
                  flex: "none",
                }}
              >
                <button
                  class={state.quotaDisplay !== "used" ? "on" : ""}
                  style={{ height: "30px", "font-size": "12.5px" }}
                  onClick={() => void update("display.quota_display_mode", "remaining")}
                >
                  {t("settings.remaining")}
                </button>
                <button
                  class={state.quotaDisplay === "used" ? "on" : ""}
                  style={{ height: "30px", "font-size": "12.5px" }}
                  onClick={() => void update("display.quota_display_mode", "used")}
                >
                  {t("settings.used")}
                </button>
              </div>
            </div>
            <div class="li">
              <div class="tx">{t("settings.accountDisplay")}</div>
              <div
                class="seg"
                style={{
                  display: "grid",
                  "grid-template-columns": "repeat(3, minmax(0, 1fr))",
                  width: "192px",
                  padding: "2px",
                  "border-radius": "9px",
                  flex: "none",
                }}
              >
                <For each={["none", "plan", "full"] as const}>
                  {(mode) => (
                    <button
                      class={state.accountDisplay === mode ? "on" : ""}
                      style={{ height: "30px", "font-size": "12px" }}
                      onClick={() => void update("display.account_display", mode)}
                    >
                      {t(mode === "none" ? "settings.accountNone" : mode === "plan" ? "settings.accountPlan" : "settings.accountFull")}
                    </button>
                  )}
                </For>
              </div>
            </div>
            <button class="li" onClick={() => void update("display.scan_antigravity", !value("display.scan_antigravity", true))}>
              <div class="tx">{t("settings.scanAntigravity")}</div>
              <span class={`tg${value("display.scan_antigravity", true) ? "" : " off"}`} />
            </button>
            <button class="li" onClick={() => void update("display.show_empty_providers", !value("display.show_empty_providers", false))}>
              <div class="tx">{t("settings.showEmptyProviders")}</div>
              <span class={`tg${value("display.show_empty_providers", false) ? "" : " off"}`} />
            </button>
          </div>
          <p class="note">{t("settings.refreshIntervalHint")}</p>

          <div class="sec">{t("settings.queriedProviders")}</div>
          <div class="tp-card" style={{ overflow: "hidden" }}>
            <For each={QUOTA_PROVIDER_KEYS}>
              {(provider) => {
                const enabled = () => state.quotaProviders[provider] !== false;
                return (
                  <button
                    class="li"
                    onClick={() =>
                      void update(`providers.${provider === "antigravity" ? "agy" : provider}.enabled`, !enabled())
                    }
                  >
                    <div class="tx">
                      {provider === "claude" ? "Claude Code" : provider === "codex" ? "Codex" : "Antigravity"}
                      <small>{enabled() ? t("settings.providerActive") : t("settings.providerDisabled")}</small>
                    </div>
                    <span class={`tg${enabled() ? "" : " off"}`} />
                  </button>
                );
              }}
            </For>
          </div>

          <div class="sec">{t("settings.notifications")}</div>
          <div class="tp-card" style={{ overflow: "hidden" }}>
            <div class="li" classList={{ off: push() === "unsupported" }}>
              <div class="tx">{t("settings.push")}</div>
              <button
                class={`tg${push() === "on" ? " on" : " off"}`}
                disabled={push() === "unsupported" || pushBusy()}
                aria-label={t("settings.push")}
                onClick={togglePush}
              />
            </div>
            <div class="li">
              <div class="tx">{t("settings.notificationLevel")}</div>
              <select
                class="inp"
                style={{ width: "132px", height: "36px", "font-size": "13px", "border-radius": "10px" }}
                value={String(value("display.notification_level", "system"))}
                onChange={(event) => void update("display.notification_level", event.currentTarget.value)}
              >
                <For each={LEVELS}>{(level) => <option value={level}>{levelLabel(level)}</option>}</For>
              </select>
            </div>
            <div class="li">
              <div class="tx">{t("settings.notificationSound")}</div>
              <select
                class="inp"
                style={{ width: "132px", height: "36px", "font-size": "13px", "border-radius": "10px" }}
                value={String(value("display.notification_sound", "chime"))}
                onChange={(event) => void update("display.notification_sound", event.currentTarget.value)}
              >
                <For each={SOUNDS}>{(sound) => <option value={sound}>{soundLabel(sound)}</option>}</For>
              </select>
            </div>
          </div>
          <Show when={push() === "unsupported"}>
            <p class="note">{t("settings.pushNotConnected")}</p>
          </Show>

          <div class="sec">{t("settings.about")}</div>
          <div class="tp-card" style={{ overflow: "hidden" }}>
            <div class="li">
              <div class="tx">{t("settings.version")}</div>
              <span class="v">{state.version}</span>
            </div>
            <Show when={!standalone()}>
              <div class="li">
                <div class="tx">
                  {t("settings.install")}
                  <small>{t("settings.installHint")}</small>
                </div>
              </div>
            </Show>
          </div>
        </div>
      </div>
    </div>
  );
}
