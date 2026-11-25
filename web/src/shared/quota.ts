// Issue #13. quota-and-usage.md §2.4.

export type QuotaLevel = "normal" | "warn" | "exhausted";

export function quotaLevel(usedPercent: number): QuotaLevel {
  if (usedPercent >= 100) return "exhausted";
  if (usedPercent >= 80) return "warn";
  return "normal";
}

export interface QuotaPaceResult {
  kind: "on_track" | "ahead" | "behind";
  expectedPercent: number;
  deficit: number;
  etaMs?: number;
}

export interface QuotaPaceWindowInput {
  usedPercent: number;
  periodMs: number | null;
  resetsAt: number | null;
}

export function quotaPace(
  window: QuotaPaceWindowInput,
  now: number = Date.now()
): QuotaPaceResult | null {
  const { usedPercent, periodMs, resetsAt } = window;
  if (usedPercent <= 0 || usedPercent >= 100) return null;
  if (periodMs === null || resetsAt === null) return null;

  const remaining = resetsAt - now;
  const elapsed = periodMs - remaining;
  if (elapsed <= 0 || remaining <= 0) return null;

  const expected = (elapsed / periodMs) * 100;
  const deficit = usedPercent - expected;

  if (Math.abs(deficit) < 5) {
    return { kind: "on_track", expectedPercent: expected, deficit };
  }
  if (deficit > 0) {
    const etaMs = (100 - usedPercent) / (usedPercent / elapsed);
    return { kind: "ahead", expectedPercent: expected, deficit, etaMs };
  }
  return { kind: "behind", expectedPercent: expected, deficit };
}

export function quotaDot(
  snapshots: Array<{ snapshot?: { windows: Array<{ usedPercent: number }> } | null }>
): "warn" | "danger" | null {
  let hasWarn = false;
  for (const s of snapshots) {
    if (!s.snapshot?.windows) continue;
    for (const w of s.snapshot.windows) {
      const lvl = quotaLevel(w.usedPercent);
      if (lvl === "exhausted") return "danger";
      if (lvl === "warn") hasWarn = true;
    }
  }
  return hasWarn ? "warn" : null;
}

