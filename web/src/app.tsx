import { createEffect, Match, onCleanup, onMount, Show, Switch } from "solid-js";
import { loadQuota, loadState, pair } from "./api";
import { KeeperPage } from "./components/KeeperPage";
import { PairPage } from "./components/PairPage";
import { QuotaPage } from "./components/QuotaPage";
import { SettingsPage } from "./components/SettingsPage";
import { UsagePage } from "./components/UsagePage";
import { showToast, state } from "./store";
import { applyTheme } from "./themes";
import { resolveLocale, setLocale, t } from "./i18n";
import { Icon } from "./components/ui";

export function App() {
  let socket: WebSocket | undefined;
  let stopped = false;
  let hadConnection = false;
  let attempt = 0;

  const connect = () => {
    if (stopped || socket || state.paired !== true) return;
    const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
    socket = new WebSocket(`${protocol}//${window.location.host}/ws`);
    socket.onopen = () => {
      state.connected = true;
      attempt = 0;
      if (hadConnection) {
        state.reconnectNonce += 1;
        void loadState().catch(() => {});
        void loadQuota().catch(() => {});
      }
      hadConnection = true;
    };
    socket.onmessage = (event) => {
      let payload: any;
      try {
        payload = JSON.parse(event.data);
      } catch {
        return;
      }
      const data = payload?.data || {};
      if (payload.type === "usage_updated") {
        state.scannedAt = data.scannedAt || state.scannedAt;
        state.usageScannedAt = data.scannedAt || Date.now();
      } else if (payload.type === "quota_updated") {
        state.quotaUpdated = data.fetchedAt || Date.now();
        void loadQuota().catch(() => {});
      } else if (payload.type === "keeper_updated") {
        state.keeperNonce += 1;
      } else if (payload.type === "settings_updated") {
        void loadState().catch(() => {});
        void loadQuota().catch(() => {});
      } else if (payload.type === "quota_recovered") {
        const provider = data.provider === "antigravity" ? "Antigravity" : data.provider;
        showToast(`${provider} ${data.window} · ${Math.round(data.remaining)}%`);
      }
    };
    socket.onclose = () => {
      socket = undefined;
      state.connected = false;
      if (stopped) return;
      attempt += 1;
      window.setTimeout(connect, Math.min(30000, 1000 * 2 ** Math.min(attempt, 5)));
    };
  };

  const initialize = async () => {
    state.bootError = "";
    try {
      const hash = window.location.hash;
      if (hash.startsWith("#token=")) {
        const token = decodeURIComponent(hash.slice("#token=".length));
        history.replaceState(null, "", window.location.pathname + window.location.search);
        await pair(token);
      } else {
        await loadState();
      }
      state.paired = true;
      await loadQuota().catch(() => {});
    } catch (error: any) {
      state.paired = false;
      if (error?.status !== 401) {
        state.bootError = error?.message || "offline";
      }
    }
  };

  const onPaired = async () => {
    state.bootError = "";
    await loadState();
    state.paired = true;
    await loadQuota().catch(() => {});
  };

  onMount(() => {
    void initialize();

    const updateOnline = () => {
      if (navigator.onLine && state.paired === true) connect();
    };
    const unauthorized = () => {
      state.paired = false;
      state.connected = false;
      state.route = { name: "usage" };
      socket?.close();
      socket = undefined;
    };
    const media = window.matchMedia("(prefers-color-scheme: light)");
    const themeChanged = () => applyTheme(state.themeMode, state.accent);

    window.addEventListener("online", updateOnline);
    window.addEventListener("offline", updateOnline);
    window.addEventListener("tokenpulse-unauthorized", unauthorized);
    media.addEventListener("change", themeChanged);
    if ("serviceWorker" in navigator) {
      void navigator.serviceWorker.register("/sw.js").catch(() => {});
    }

    onCleanup(() => {
      stopped = true;
      socket?.close();
      socket = undefined;
      window.removeEventListener("online", updateOnline);
      window.removeEventListener("offline", updateOnline);
      window.removeEventListener("tokenpulse-unauthorized", unauthorized);
      media.removeEventListener("change", themeChanged);
    });
  });

  createEffect(() => {
    applyTheme(state.themeMode, state.accent);
  });

  createEffect(() => {
    const pref = state.localePref;
    const languages = navigator.languages?.length ? navigator.languages : [navigator.language];
    setLocale(resolveLocale(pref, languages));
  });

  createEffect(() => {
    if (state.paired === true) {
      connect();
    }
  });

  return (
    <>
      <Show when={state.paired === null}>
        <div class="page">
          <div class="scroll" style={{ display: "flex", "align-items": "center", "justify-content": "center" }}>
            <div class="spin" style={{ width: "22px", height: "22px" }} />
          </div>
        </div>
      </Show>
      <Show when={state.paired === false}>
        <PairPage onPaired={onPaired} />
      </Show>
      <Show when={state.paired === true}>
        <Switch>
          <Match when={state.route.name === "usage"}>
            <UsagePage />
          </Match>
          <Match when={state.route.name === "quota"}>
            <QuotaPage />
          </Match>
          <Match when={state.route.name === "keeper"}>
            <KeeperPage />
          </Match>
          <Match when={state.route.name === "settings"}>
            <SettingsPage />
          </Match>
        </Switch>
        <nav class="bottom-nav">
          <button
            classList={{ active: state.route.name === "usage" }}
            onClick={() => (state.route = { name: "usage" })}
          >
            <span><Icon name="barChart" /></span>
            <em>{t("home.usage")}</em>
          </button>
          <button
            classList={{ active: state.route.name === "quota" }}
            onClick={() => (state.route = { name: "quota" })}
          >
            <span><Icon name="gauge" /></span>
            <em>{t("home.quota")}</em>
          </button>
          <button
            classList={{ active: state.route.name === "keeper" }}
            onClick={() => (state.route = { name: "keeper" })}
          >
            <span><Icon name="clock" /></span>
            <em>{t("home.keeper")}</em>
          </button>
          <button
            classList={{ active: state.route.name === "settings" }}
            onClick={() => (state.route = { name: "settings" })}
          >
            <span><Icon name="gear" /></span>
            <em>{t("home.settings")}</em>
          </button>
        </nav>
      </Show>
      <Show when={state.toast}>
        <div class="toast">{state.toast}</div>
      </Show>
    </>
  );
}
