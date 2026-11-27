import type { UsageGetPayload, UsagePreset } from "../types";

export const BOOT_CACHE_VERSION = 1;

const STATE_KEY = `tokenpulse.boot.v${BOOT_CACHE_VERSION}.state`;
const USAGE_KEY = `tokenpulse.boot.v${BOOT_CACHE_VERSION}.usage`;
const PAIRED_KEY = "tokenpulse.paired";

export interface CachedState {
  version: string;
  scannedAt: number;
  fetchedAt: number;
  agents: string[];
  settings: Record<string, unknown>;
  keeperEngine: boolean;
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

function isRecord(value: unknown): value is Record<string, unknown> {
  return Boolean(value) && typeof value === "object" && !Array.isArray(value);
}

function isCachedState(value: unknown): value is CachedState {
  if (!isRecord(value)) return false;
  return (
    typeof value.version === "string" &&
    typeof value.scannedAt === "number" &&
    typeof value.fetchedAt === "number" &&
    Array.isArray(value.agents) &&
    value.agents.every((agent) => typeof agent === "string") &&
    isRecord(value.settings) &&
    typeof value.keeperEngine === "boolean"
  );
}

function isUsageReport(value: unknown): value is UsageGetPayload {
  if (!isRecord(value) || !isRecord(value.range) || !isRecord(value.totals)) return false;
  return (
    typeof value.range.from === "string" &&
    typeof value.range.to === "string" &&
    typeof value.scannedAt === "number" &&
    typeof value.totals.cost === "number" &&
    Array.isArray(value.days) &&
    Array.isArray(value.models)
  );
}

function isCachedUsage(value: unknown): value is CachedUsage {
  if (!isRecord(value)) return false;
  return (
    typeof value.preset === "string" &&
    Array.isArray(value.agents) &&
    value.agents.every((agent) => typeof agent === "string") &&
    typeof value.savedAt === "number" &&
    isUsageReport(value.report)
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

export function readCachedUsage(): CachedUsage | null {
  return decode(readRaw(USAGE_KEY), isCachedUsage);
}

export function writeCachedUsage(value: CachedUsage): void {
  writeRaw(USAGE_KEY, encode(value));
}

export function clearBootCache(): void {
  removeRaw(STATE_KEY);
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
