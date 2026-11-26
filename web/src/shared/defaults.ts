// Defaults shared by the phone app. TokenPulse names Antigravity `agy` on the wire.

import type { UsagePreset } from "../types";

export type UsageScanInterval = "1m" | "2m" | "5m" | "10m" | "15m";

/** Quota providers in display order, in TokenPulse's internal names. */
export const QUOTA_PROVIDER_KEYS = ["claude", "codex", "antigravity"] as const;

/** The seven agents the API can report. */
export const USAGE_AGENT_KINDS = ["claude", "codex", "copilot", "gemini", "opencode", "pi", "agy"] as const;

export const AGENT_NAMES: Record<string, string> = {
  claude: "Claude Code",
  codex: "Codex",
  copilot: "Copilot",
  gemini: "Gemini",
  agy: "Antigravity",
  antigravity: "Antigravity",
  opencode: "OpenCode",
  pi: "Pi",
};

export const USAGE_PRESETS: UsagePreset[] = ["today", "7d", "30d", "year", "all", "custom"];

const SCAN_INTERVAL_MS: Record<UsageScanInterval, number> = {
  "1m": 60_000,
  "2m": 120_000,
  "5m": 300_000,
  "10m": 600_000,
  "15m": 900_000,
};

export const SCAN_INTERVALS = Object.keys(SCAN_INTERVAL_MS) as UsageScanInterval[];

/** The server's refresh interval is in seconds; 0 falls back to the 5m default. */
export function secondsToScanInterval(seconds: number): UsageScanInterval {
  switch (seconds) {
    case 60:
      return "1m";
    case 120:
      return "2m";
    case 300:
      return "5m";
    case 600:
      return "10m";
    case 900:
      return "15m";
    default:
      return "5m";
  }
}

export function scanIntervalToMs(interval?: UsageScanInterval): number {
  return (interval && SCAN_INTERVAL_MS[interval]) || SCAN_INTERVAL_MS["5m"];
}
