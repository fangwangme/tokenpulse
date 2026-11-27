import type { QuotaProviderState, UsageGetPayload, UsagePreset } from "../types";

export const BOOT_CACHE_VERSION = 1;

const STATE_KEY = `tokenpulse.boot.v${BOOT_CACHE_VERSION}.state`;
const QUOTA_KEY = `tokenpulse.boot.v${BOOT_CACHE_VERSION}.quota`;
const USAGE_KEY = `tokenpulse.boot.v${BOOT_CACHE_VERSION}.usage`;
const PAIRED_KEY = "tokenpulse.paired";

export interface CachedState {
  version: string;
  scannedAt: number;
  fetchedAt: number;
  pricesAvailable: boolean;
  agents: string[];
  settings: Record<string, unknown>;
  keeperEngine: boolean;
}

export interface CachedQuota {
  providers: QuotaProviderState[];
  fetchedAt: number | null;
}

export interface CachedUsage {
  preset: UsagePreset;
  agents: string[];
  report: UsageGetPayload;
  savedAt: number;
}

function storage(): Storage | null {
  try {
    return typeof localStorage === "undefined" ? null : localStorage;
  } catch {
    return null;
  }
}

function readRaw(key: string): string | null {
  try {
    return storage()?.getItem(key) ?? null;
  } catch {
    return null;
  }
}

function writeRaw(key: string, value: string): void {
  try {
    storage()?.setItem(key, value);
  } catch {
    return;
  }
}

function removeRaw(key: string): void {
  try {
    storage()?.removeItem(key);
  } catch {
    return;
  }
}

function encode<T>(data: T): string {
  return JSON.stringify({ version: BOOT_CACHE_VERSION, data });
}

function decode<T>(raw: string | null | undefined, valid: (value: unknown) => value is T): T | null {
  if (!raw) return null;
  let parsed: unknown;
  try {
    parsed = JSON.parse(raw);
  } catch {
    return null;
  }
  if (!parsed || typeof parsed !== "object") return null;
  const envelope = parsed as { version?: unknown; data?: unknown };
  if (envelope.version !== BOOT_CACHE_VERSION) return null;
  return valid(envelope.data) ? envelope.data : null;
}

function isCachedState(value: unknown): value is CachedState {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<CachedState>;
  return (
    typeof entry.version === "string" &&
    typeof entry.scannedAt === "number" &&
    typeof entry.fetchedAt === "number" &&
    typeof entry.pricesAvailable === "boolean" &&
    Array.isArray(entry.agents) &&
    entry.agents.every((agent) => typeof agent === "string") &&
    Boolean(entry.settings) &&
    typeof entry.settings === "object" &&
    !Array.isArray(entry.settings) &&
    typeof entry.keeperEngine === "boolean"
  );
}

function isQuotaProviderState(value: unknown): value is QuotaProviderState {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<QuotaProviderState>;
  if (typeof entry.provider !== "string" || typeof entry.enabled !== "boolean") return false;
  if (entry.error !== null) {
    if (!entry.error || typeof entry.error !== "object") return false;
    if (typeof (entry.error as { message?: unknown }).message !== "string") return false;
  }
  if (entry.snapshot === null) return true;
  const snapshot = entry.snapshot;
  return (
    Boolean(snapshot) &&
    typeof snapshot!.provider === "string" &&
    typeof snapshot!.fetchedAt === "number" &&
    Array.isArray(snapshot!.windows) &&
    snapshot!.windows.every(
      (window) => Boolean(window) && typeof window.id === "string" && typeof window.usedPercent === "number",
    )
  );
}

function isCachedQuota(value: unknown): value is CachedQuota {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<CachedQuota>;
  return (
    Array.isArray(entry.providers) &&
    entry.providers.every(isQuotaProviderState) &&
    (entry.fetchedAt === null || typeof entry.fetchedAt === "number")
  );
}

function isUsageReport(value: unknown): value is UsageGetPayload {
  if (!value || typeof value !== "object") return false;
  const report = value as Partial<UsageGetPayload>;
  return (
    Boolean(report.range) &&
    typeof report.range?.from === "string" &&
    typeof report.range?.to === "string" &&
    typeof report.scannedAt === "number" &&
    Boolean(report.totals) &&
    typeof report.totals?.cost === "number" &&
    Array.isArray(report.days) &&
    report.days.every(
      (day) => Boolean(day) && typeof day.date === "string" && typeof day.tokens === "number" && typeof day.cost === "number",
    ) &&
    Array.isArray(report.models) &&
    report.models.every(
      (model) =>
        Boolean(model) && typeof model.model === "string" && typeof model.tokens === "number" && typeof model.cost === "number",
    )
  );
}

function isCachedUsage(value: unknown): value is CachedUsage {
  if (!value || typeof value !== "object") return false;
  const entry = value as Partial<CachedUsage>;
  return (
    typeof entry.preset === "string" &&
    Array.isArray(entry.agents) &&
    entry.agents.every((agent) => typeof agent === "string") &&
    typeof entry.savedAt === "number" &&
    isUsageReport(entry.report)
  );
}

export function sameAgentSet(left: readonly string[], right: readonly string[]): boolean {
  if (left.length !== right.length) return false;
  const seen = new Set(left);
  return seen.size === left.length && new Set(right).size === right.length && right.every((agent) => seen.has(agent));
}

export function cachedUsageMatches(cache: CachedUsage | null, preset: string, agents: readonly string[]): boolean {
  return Boolean(cache) && cache!.preset === preset && sameAgentSet(cache!.agents, agents);
}

export function parseCachedUsage(raw: string | null | undefined): CachedUsage | null {
  return decode(raw, isCachedUsage);
}

export function serializeCachedUsage(value: CachedUsage): string {
  return encode(value);
}

export function readCachedState(): CachedState | null {
  return decode(readRaw(STATE_KEY), isCachedState);
}

export function writeCachedState(value: CachedState): void {
  writeRaw(STATE_KEY, encode(value));
}

export function readCachedQuota(): CachedQuota | null {
  return decode(readRaw(QUOTA_KEY), isCachedQuota);
}

export function writeCachedQuota(value: CachedQuota): void {
  writeRaw(QUOTA_KEY, encode(value));
}

export function readCachedUsage(): CachedUsage | null {
  return decode(readRaw(USAGE_KEY), isCachedUsage);
}

export function writeCachedUsage(value: CachedUsage): void {
  writeRaw(USAGE_KEY, encode(value));
}

export function clearBootCache(): void {
  removeRaw(STATE_KEY);
  removeRaw(QUOTA_KEY);
  removeRaw(USAGE_KEY);
}

export function readPairingMarker(): boolean {
  return readRaw(PAIRED_KEY) === "1";
}

export function writePairingMarker(): void {
  writeRaw(PAIRED_KEY, "1");
}

export function clearPairingMarker(): void {
  removeRaw(PAIRED_KEY);
}
