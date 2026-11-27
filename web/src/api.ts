import { rememberPairing, showToast, state } from "./store";
import { t } from "./i18n";
import { QUOTA_PROVIDER_KEYS, secondsToScanInterval } from "./shared/defaults";
import type {
  QuotaGetPayload,
  QuotaProviderState,
  UsageDayAgent,
  UsageDayPayload,
  UsageGetPayload,
  UsageModelPayload,
} from "./types";
import type { KeeperPayload } from "./store";

const jsonHeaders = { "Content-Type": "application/json" };

function normalizeProviderOrder(value: unknown): string[] {
  const known = new Set<string>(QUOTA_PROVIDER_KEYS);
  const seen = new Set<string>();
  const order: string[] = [];
  if (Array.isArray(value)) {
    for (const item of value) {
      if (typeof item === "string" && known.has(item) && !seen.has(item)) {
        seen.add(item);
        order.push(item);
      }
    }
  }
  for (const id of QUOTA_PROVIDER_KEYS) {
    if (!seen.has(id)) order.push(id);
  }
  return order;
}

export class ApiError extends Error {
  status: number;
  constructor(message: string, status: number) {
    super(message);
    this.status = status;
  }
}

async function api<T>(path: string, options: RequestInit = {}): Promise<T> {
  const response = await fetch(path, {
    credentials: "same-origin",
    ...options,
    headers: {
      ...(options.body ? jsonHeaders : {}),
      ...(options.headers || {}),
    },
  });
  const text = await response.text();
  let data: any = null;
  if (text) {
    try {
      data = JSON.parse(text);
    } catch {
      data = null;
    }
  }
  if (response.status === 401 && path !== "/api/pair") {
    window.dispatchEvent(new Event("tokenpulse-unauthorized"));
  }
  if (!response.ok) {
    throw new ApiError(data?.error || response.statusText || "request failed", response.status);
  }
  return data as T;
}

interface RawState {
  version: string;
  scannedAt: number;
  fetchedAt: number;
  pricesAvailable: boolean;
  agents: string[];
  settings: Record<string, unknown>;
  keeperEngine: boolean;
}

interface RawUsageDay {
  date: string;
  totals: UsageDayPayload["totals"];
  agents: Array<{
    agent: string;
    totals: UsageDayPayload["totals"];
    models: UsageDayPayload["agents"][number]["models"];
  }>;
}

interface RawQuotaWindow {
  id: string;
  label: string;
  family?: string;
  usedPercent: number;
  periodMs?: number;
  resetsAt?: string;
}

interface RawQuotaSnapshot {
  provider: string;
  plan?: string;
  account?: string;
  windows: RawQuotaWindow[];
  credits?: { used: number; limit?: number; currency: string };
  extraUsage?: { used: number; limit?: number; currency: string };
  rateLimitResetCredits?: Array<{ id: string; resetType?: string; expiresAt?: string }>;
  fetchedAt: string;
}

interface RawQuota {
  providers: Array<{
    provider: string;
    enabled: boolean;
    snapshot: RawQuotaSnapshot | null;
    error: { message: string; at: string } | null;
  }>;
  fetchedAt: number;
}

function millis(value: string | null | undefined): number | null {
  if (!value) return null;
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

function normalizeProvider(provider: string): string {
  return provider === "agy" ? "antigravity" : provider;
}

function normalizeUsage(payload: UsageGetPayload): UsageGetPayload {
  return {
    ...payload,
    comparison: payload.comparison
      ? {
          from: payload.comparison.from,
          to: payload.comparison.to,
          untilMinutes: payload.comparison.untilMinutes,
          totals: payload.comparison.totals,
        }
      : null,
  };
}

export async function pair(token: string): Promise<void> {
  await api("/api/pair", { method: "POST", body: JSON.stringify({ token }) });
  await loadState();
}

function applySettings(values: Record<string, unknown>): void {
  state.settings = values;
  state.quotaDisplay = values["display.quota_display_mode"] === "used" ? "used" : "remaining";
  state.accountDisplay =
    values["display.account_display"] === "none"
      ? "none"
      : values["display.account_display"] === "plan"
        ? "plan"
        : "full";
  state.quotaShowAccount = state.accountDisplay !== "none";
  state.usageScanInterval = secondsToScanInterval(Number(values["display.auto_refresh_secs"] ?? 900));
  state.quotaProviders = {
    claude: values["providers.claude.enabled"] !== false,
    codex: values["providers.codex.enabled"] !== false,
    opencode: values["providers.opencode.enabled"] !== false,
    antigravity: values["providers.agy.enabled"] !== false,
  };
  state.quotaProviderOrder = normalizeProviderOrder(values["display.quota_provider_order"]);
  state.quotaEnabled = values["display.refresh_quota"] !== false;
}

let stateInFlight: Promise<void> | null = null;

async function performStateLoad(): Promise<void> {
  const epoch = state.authEpoch;
  const raw = await api<RawState>("/api/state");
  if (epoch !== state.authEpoch) return;
  state.version = raw.version;
  state.scannedAt = raw.scannedAt;
  state.usageScannedAt = raw.scannedAt;
  state.agents = raw.agents || [];
  state.keeperEngine = raw.keeperEngine;
  state.quotaUpdated = raw.fetchedAt || null;
  applySettings(raw.settings || {});
  state.usageEnabled = true;
  state.configReady = true;
  if (state.paired !== true) state.paired = true;
  rememberPairing();
}

export function loadState(): Promise<void> {
  if (stateInFlight) return stateInFlight;
  const run = performStateLoad();
  stateInFlight = run;
  const clear = () => {
    if (stateInFlight === run) stateInFlight = null;
  };
  void run.then(clear, clear);
  return run;
}

interface RefreshResult {
  accepted: boolean;
  throttled: boolean;
  retryAfter: number;
}

async function manualRefresh(flags: { usage: boolean; quota: boolean }): Promise<RefreshResult> {
  const result = await api<RefreshResult>("/api/refresh", {
    method: "POST",
    body: JSON.stringify(flags),
  });
  if (result.throttled) {
    showToast(t("refresh.cached", { secs: Math.max(1, result.retryAfter) }));
  }
  return result;
}

export async function refreshUsage(): Promise<void> {
  await manualRefresh({ usage: true, quota: false });
}

export async function request(message: {
  type: "usage_get" | "usage_day" | "usage_model";
  payload: any;
}): Promise<UsageGetPayload | UsageDayPayload | UsageModelPayload> {
  if (message.type === "usage_get") {
    const payload = message.payload as {
      preset: string;
      from?: string;
      to?: string;
      agents: string[];
      force?: boolean;
    };
    if (payload.force) {
      await refreshUsage();
    }
    const params = new URLSearchParams({ preset: payload.preset, agents: payload.agents.join(",") });
    if (payload.from) params.set("from", payload.from);
    if (payload.to) params.set("to", payload.to);
    return normalizeUsage(await api<UsageGetPayload>(`/api/usage?${params}`));
  }
  if (message.type === "usage_day") {
    const payload = message.payload as { date: string; agents: string[] };
    const raw = await api<RawUsageDay>(
      `/api/usage/day?${new URLSearchParams({ date: payload.date, agents: payload.agents.join(",") })}`,
    );
    return {
      date: raw.date,
      totals: raw.totals,
      agents: raw.agents.map(
        (agent): UsageDayAgent => ({
          agent: agent.agent,
          ...agent.totals,
          models: agent.models,
        }),
      ),
    };
  }
  const payload = message.payload as { model: string; from: string; to: string; agents: string[] };
  return api<UsageModelPayload>(
    `/api/usage/model?${new URLSearchParams({
      model: payload.model,
      from: payload.from,
      to: payload.to,
      agents: payload.agents.join(","),
    })}`,
  );
}

function mapQuota(raw: RawQuota): QuotaProviderState[] {
  return raw.providers.map((entry) => {
    const provider = normalizeProvider(entry.provider);
    return {
      provider,
      enabled: entry.enabled,
      error: entry.error
        ? { message: entry.error.message, at: millis(entry.error.at) ?? Date.now() }
        : null,
      snapshot: entry.snapshot
        ? {
            provider,
            plan: entry.snapshot.plan,
            account: entry.snapshot.account,
            fetchedAt: millis(entry.snapshot.fetchedAt) ?? Date.now(),
            windows: entry.snapshot.windows.map((window) => ({
              id: window.id,
              label: window.label,
              family: window.family,
              usedPercent: window.usedPercent,
              periodMs: window.periodMs ?? null,
              resetsAt: millis(window.resetsAt),
            })),
            credits: (entry.snapshot.rateLimitResetCredits || []).map((credit) => ({
              id: credit.id,
              label: credit.resetType || credit.id,
              expiresAt: millis(credit.expiresAt),
            })),
            extraUsage: entry.snapshot.extraUsage
              ? {
                  used: entry.snapshot.extraUsage.used,
                  limit: entry.snapshot.extraUsage.limit ?? null,
                  currency: entry.snapshot.extraUsage.currency,
                }
              : undefined,
          }
        : null,
    };
  });
}

let quotaInFlight: Promise<void> | null = null;
let quotaLoads = 0;

async function performQuotaLoad(refresh: boolean): Promise<void> {
  if (refresh) {
    const res = await manualRefresh({ usage: false, quota: true });
    if (res.throttled && !res.accepted && state.quotaSnapshots.length > 0) {
      return;
    }
  }
  const raw = await api<RawQuota>("/api/quota");
  const providers = mapQuota(raw);
  state.quotaSnapshots = providers;
  if (raw.fetchedAt) state.quotaUpdated = raw.fetchedAt;
}

export function loadQuota(refresh = false): Promise<void> {
  if (!refresh && quotaInFlight) return quotaInFlight;
  quotaLoads += 1;
  state.quotaLoading = true;
  const run = performQuotaLoad(refresh).finally(() => {
    quotaLoads -= 1;
    if (quotaLoads === 0) state.quotaLoading = false;
  });
  if (refresh) return run;
  quotaInFlight = run;
  const clear = () => {
    if (quotaInFlight === run) quotaInFlight = null;
  };
  void run.then(clear, clear);
  return run;
}

export async function fetchQuota(refresh = false): Promise<void> {
  return loadQuota(refresh);
}

export async function patchSettings(updates: Record<string, unknown>): Promise<void> {
  const raw = await api<{ settings: Record<string, { value: unknown }> }>("/api/settings", {
    method: "PATCH",
    body: JSON.stringify(updates),
  });
  const values: Record<string, unknown> = {};
  for (const [key, entry] of Object.entries(raw.settings || {})) values[key] = entry.value;
  applySettings(values);
  state.keeperEngine = Boolean(values["keeper.enabled"]);
}

export async function loadKeeper(): Promise<KeeperPayload> {
  return api<KeeperPayload>("/api/keeper");
}

export async function pingKeeper(agent: string, group?: string): Promise<void> {
  await api("/api/keeper/ping", {
    method: "POST",
    body: JSON.stringify(group ? { agent, group } : { agent }),
  });
}

export function getPushKey(): Promise<{ publicKey: string }> {
  return api("/api/push/key");
}

export async function addPushSubscription(subscription: PushSubscriptionJSON): Promise<void> {
  await api("/api/push/subscription", { method: "POST", body: JSON.stringify(subscription) });
}

export async function removePushSubscription(endpoint: string): Promise<void> {
  await api("/api/push/subscription", { method: "DELETE", body: JSON.stringify({ endpoint }) });
}
