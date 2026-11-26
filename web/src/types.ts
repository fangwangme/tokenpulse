export type ThemeMode = "system" | "light" | "dark";
export type QuotaDisplayMode = "remaining" | "used";

export type UsagePreset = "today" | "7d" | "30d" | "year" | "all" | "custom";

export interface UsageTotals {
  cost: number;
  tokens: number;
  input: number;
  output: number;
  cacheRead: number;
  cacheWrite: number;
  messages: number;
  sessions: number;
}

export interface UsageDaySummary extends UsageTotals {
  date: string;
}

export interface UsageModelSummary extends UsageTotals {
  model: string;
  company: string;
  agents: string[];
  lastUsed: string;
  unpriced: boolean;
}

export interface UsageComparison {
  from: string;
  to: string;
  untilMinutes?: number;
  totals: UsageTotals;
}

export interface UsageGetPayload {
  range: { from: string; to: string };
  firstDay: string | null;
  scannedAt: number;
  pricesAvailable: boolean;
  totals: UsageTotals;
  comparison: UsageComparison | null;
  days: UsageDaySummary[];
  models: UsageModelSummary[];
}

export interface UsageDayAgentModel extends UsageTotals {
  model: string;
  company?: string;
}

export interface UsageDayAgent extends UsageTotals {
  agent: string;
  models: UsageDayAgentModel[];
}

export interface UsageDayPayload {
  date: string;
  totals: UsageTotals;
  agents: UsageDayAgent[];
}

export interface UsageModelPerDay {
  date: string;
  tokens: number;
  cost: number;
}

export interface UsageModelAgentTotals extends UsageTotals {
  agent: string;
}

export interface UsageModelPayload {
  model: string;
  company: string;
  totals: UsageTotals;
  lastUsed: string;
  agents: UsageModelAgentTotals[];
  perDay: UsageModelPerDay[];
}

export interface QuotaWindow {
  id: string;
  label: string;
  family?: string;
  usedPercent: number;
  periodMs: number | null;
  resetsAt: number | null;
}

export interface QuotaCredit {
  id?: string;
  label?: string;
  expiresAt: number | null;
  [key: string]: unknown;
}

export interface QuotaExtraUsage {
  used: number;
  limit: number | null;
  currency?: string;
}

export interface QuotaSnapshot {
  provider: string;
  plan?: string;
  account?: string;
  windows: QuotaWindow[];
  credits: QuotaCredit[];
  extraUsage?: QuotaExtraUsage;
  fetchedAt: number;
}

export interface QuotaProviderState {
  provider: string;
  enabled: boolean;
  snapshot: QuotaSnapshot | null;
  error: { message: string; at: number } | null;
}

export interface QuotaGetPayload {
  providers: QuotaProviderState[];
  fetchedAt?: number;
}

export interface ModelRecipeSummary {
  models: Array<{ id: string; label?: string }>;
}

export type RouteName = "home" | "usage" | "quota" | "keeper" | "settings";

export interface Route {
  name: RouteName;
  initialPreset?: UsagePreset;
  from?: RouteName;
}
