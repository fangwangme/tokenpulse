/** usage-report.md "Numbers": the one money format every phone surface uses. */
export function formatCost(value: number): string {
  if (value >= 1_000_000) return `$${(value / 1_000_000).toFixed(3)}M`;
  if (value >= 1_000) return `$${(value / 1_000).toFixed(3)}K`;
  return `$${value.toFixed(2)}`;
}

/** Quota extra usage keeps its currency; USD uses the compact money format. */
export function formatMoney(value: number, currency = "USD"): string {
  const code = (currency || "USD").toUpperCase();
  if (code === "USD") return formatCost(value);
  if (value >= 1_000_000) return `${(value / 1_000_000).toFixed(3)}M ${code}`;
  if (value >= 1_000) return `${(value / 1_000).toFixed(3)}K ${code}`;
  return `${value.toFixed(2)} ${code}`;
}
