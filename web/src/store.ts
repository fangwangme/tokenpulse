import { createMutable } from "solid-js/store";
import type { QuotaProviderState, Route, ThemeMode } from "./types";
import { QUOTA_PROVIDER_KEYS, type UsageScanInterval } from "./shared/defaults";
import {
  clearBootCache,
  clearPairingMarker,
  readPairingMarker,
  writePairingMarker,
} from "./shared/boot-cache";
import type { LocalePref } from "./i18n";

export interface KeeperExecution {
  time: string;
  agent: string;
  trigger: string;
  status: "success" | "failed";
  model: string;
  reply: string;
}

export interface KeeperTarget {
  key: string;
  group?: string;
  lastFired: string | null;
}

export interface KeeperAgent {
  sessionKeeperEnabled: boolean;
  weeklyKeeperEnabled: boolean;
  dailyWakeupTime: string;
  weeklyAllGroups: boolean;
  running: boolean;
  targets: KeeperTarget[];
}

export interface KeeperPayload {
  engine: boolean;
  agents: Record<string, KeeperAgent>;
  executions: KeeperExecution[];
}

interface AppState {
  paired: boolean | null;
  bootError: string;
  connected: boolean;
  everConnected: boolean;
  configReady: boolean;
  version: string;
  scannedAt: number;
  usageScannedAt: number;
  usageEnabled: boolean;
  quotaEnabled: boolean;
  quotaLoading: boolean;
  quotaUpdated: number | null;
  quotaSnapshots: QuotaProviderState[];
  quotaProviders: Record<string, boolean>;
  quotaProviderOrder: string[];
  quotaDisplay: "remaining" | "used";
  accountDisplay: "none" | "plan" | "full";
  quotaShowAccount: boolean;
  usageScanInterval: UsageScanInterval;
  settings: Record<string, unknown>;
  agents: string[];
  keeperEngine: boolean;
  themeMode: ThemeMode;
  localePref: LocalePref;
  route: Route;
  reconnectNonce: number;
  keeperNonce: number;
  toast: string;
  navHidden: boolean;
}

function stored<T extends string>(key: string, allowed: readonly T[], fallback: T): T {
  if (typeof localStorage === "undefined") return fallback;
  const value = localStorage.getItem(key);
  return (allowed as readonly string[]).includes(value ?? "") ? (value as T) : fallback;
}

if (typeof localStorage !== "undefined") localStorage.removeItem("tokenpulse-accent");

export const state = createMutable<AppState>({
  paired: readPairingMarker() ? true : null,
  bootError: "",
  connected: false,
  everConnected: false,
  configReady: false,
  version: "",
  scannedAt: 0,
  usageScannedAt: 0,
  usageEnabled: true,
  quotaEnabled: true,
  quotaLoading: false,
  quotaUpdated: null,
  quotaSnapshots: [],
  quotaProviders: { claude: true, codex: true, opencode: true, antigravity: true },
  quotaProviderOrder: [...QUOTA_PROVIDER_KEYS],
  quotaDisplay: "remaining",
  accountDisplay: "full",
  quotaShowAccount: true,
  usageScanInterval: "15m",
  settings: {},
  agents: [],
  keeperEngine: false,
  themeMode: stored("tokenpulse-theme", ["system", "light", "dark"] as const, "system"),
  localePref: stored("tokenpulse-locale", ["system", "en", "zh"] as const, "system"),
  route: { name: "usage" },
  reconnectNonce: 0,
  keeperNonce: 0,
  toast: "",
  navHidden: false,
});

export function rememberPairing(): void {
  writePairingMarker();
}

export function forgetPairing(): void {
  clearPairingMarker();
  clearBootCache();
}

export function navigate(route: Route): void {
  state.route = route;
  state.navHidden = false;
  if (typeof window !== "undefined") {
    window.scrollTo(0, 0);
  }
}

export function createScrollNavHandler(): (e: Event & { currentTarget: HTMLElement }) => void {
  let lastTop = 0;
  return (e) => {
    const el = e.currentTarget;
    const top = Math.max(0, el.scrollTop);
    const maxScroll = el.scrollHeight - el.clientHeight;
    if (maxScroll <= 40 || top <= 24 || top >= maxScroll - 24) {
      if (state.navHidden) state.navHidden = false;
      lastTop = top;
      return;
    }
    const delta = top - lastTop;
    if (delta > 8 && top > 56) {
      if (!state.navHidden) state.navHidden = true;
      lastTop = top;
    } else if (delta < -8) {
      if (state.navHidden) state.navHidden = false;
      lastTop = top;
    }
  };
}

export function showToast(message: string): void {
  state.toast = message;
  window.setTimeout(() => {
    if (state.toast === message) state.toast = "";
  }, 3200);
}

export function setting<T>(key: string, fallback: T): T {
  const value = state.settings[key];
  return (value === undefined || value === null ? fallback : value) as T;
}

export function saveTheme(mode: ThemeMode): void {
  state.themeMode = mode;
  localStorage.setItem("tokenpulse-theme", mode);
}

export function saveLocale(pref: LocalePref): void {
  state.localePref = pref;
  localStorage.setItem("tokenpulse-locale", pref);
}
