// Issue #13. quota-and-usage.md §3.2, §3.4–§3.6.

export type UsagePreset = "today" | "7d" | "30d" | "year" | "all" | "custom";

function parseYMD(ymd: string): [number, number, number] {
  const [y, m, d] = ymd.split("-").map(Number);
  return [y, m, d];
}

/** A local date as `YYYY-MM-DD` (§3.2: dates are in the computer's time zone). */
export function formatYMD(date: Date): string {
  const y = date.getFullYear();
  const m = String(date.getMonth() + 1).padStart(2, "0");
  const d = String(date.getDate()).padStart(2, "0");
  return `${y}-${m}-${d}`;
}

export function addDays(ymd: string, days: number): string {
  const [y, m, d] = parseYMD(ymd);
  const dt = new Date(y, m - 1, d + days);
  return formatYMD(dt);
}

export function daysBetween(fromYmd: string, toYmd: string): number {
  const [y1, m1, d1] = parseYMD(fromYmd);
  const [y2, m2, d2] = parseYMD(toYmd);
  const dt1 = new Date(y1, m1 - 1, d1);
  const dt2 = new Date(y2, m2 - 1, d2);
  return Math.round((dt2.getTime() - dt1.getTime()) / (24 * 60 * 60 * 1000));
}

export interface ResolveRangeOptions {
  firstDay?: string | null;
  from?: string;
  to?: string;
}

export function resolveRange(
  preset: UsagePreset,
  today: string,
  options: ResolveRangeOptions = {}
): { from: string; to: string } {
  switch (preset) {
    case "today":
      return { from: today, to: today };
    case "7d":
      return { from: addDays(today, -6), to: today };
    case "30d":
      return { from: addDays(today, -29), to: today };
    case "year": {
      const year = today.slice(0, 4);
      return { from: `${year}-01-01`, to: today };
    }
    case "all":
      return { from: options.firstDay ?? today, to: today };
    case "custom": {
      const f = options.from ?? today;
      const t = options.to ?? today;
      if (f > t) {
        return { from: t, to: f };
      }
      return { from: f, to: t };
    }
  }
}

export function comparisonRange(
  range: { from: string; to: string },
  preset: UsagePreset,
  today: string,
  firstDay: string | null,
  nowMinutes: number
): { from: string; to: string; untilMinutes: number | null } | null {
  if (preset === "year" || preset === "all") return null;

  const isSingle = range.from === range.to;
  let compFrom: string;
  let compTo: string;
  let untilMinutes: number | null = null;

  if (isSingle) {
    if (range.from === today) {
      compFrom = addDays(today, -7);
      compTo = compFrom;
      untilMinutes = nowMinutes;
    } else {
      compFrom = addDays(range.from, -7);
      compTo = compFrom;
      untilMinutes = null;
    }
  } else {
    const len = daysBetween(range.from, range.to) + 1;
    compTo = addDays(range.from, -1);
    compFrom = addDays(range.from, -len);
    untilMinutes = null;
  }

  if (firstDay && compFrom < firstDay) {
    return null;
  }

  return { from: compFrom, to: compTo, untilMinutes };
}

export function heatmapLayout(range: { from: string; to: string }): "strip" | "week-rows" | "week-columns" {
  const len = daysBetween(range.from, range.to) + 1;
  if (len <= 7) return "strip";

  const [y1, m1, d1] = parseYMD(range.from);
  const [y2, m2, d2] = parseYMD(range.to);
  const dt1 = new Date(y1, m1 - 1, d1);
  const dt2 = new Date(y2, m2 - 1, d2);

  // Monday-based day of week: Monday is 0, Sunday is 6
  const dow1 = (dt1.getDay() + 6) % 7;
  const dow2 = (dt2.getDay() + 6) % 7;

  // Monday of the week containing range.from
  const startMonday = new Date(y1, m1 - 1, d1 - dow1);
  // Sunday of the week containing range.to
  const endSunday = new Date(y2, m2 - 1, d2 + (6 - dow2));

  const totalDays = Math.round((endSunday.getTime() - startMonday.getTime()) / (24 * 60 * 60 * 1000)) + 1;
  const weeks = Math.round(totalDays / 7);

  if (weeks <= 6) return "week-rows";
  return "week-columns";
}

export function cacheHitPercent(cacheRead: number, totalTokens: number): number {
  if (totalTokens <= 0) return 0;
  return Number(((cacheRead / totalTokens) * 100).toFixed(2));
}
