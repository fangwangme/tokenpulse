import assert from "node:assert/strict";
import { test } from "node:test";
import {
  BOOT_CACHE_VERSION,
  cachedUsageMatches,
  parseCachedUsage,
  serializeCachedUsage,
  type CachedUsage,
} from "../src/shared/boot-cache.ts";
import type { UsageGetPayload } from "../src/types.ts";

const report: UsageGetPayload = {
  range: { from: "2026-11-27", to: "2026-11-27" },
  firstDay: "2026-11-27",
  scannedAt: 1_764_000_000_000,
  pricesAvailable: true,
  totals: {
    cost: 1,
    tokens: 2,
    input: 3,
    output: 4,
    cacheRead: 5,
    cacheWrite: 6,
    messages: 7,
    sessions: 8,
  },
  comparison: null,
  days: [
    {
      date: "2026-11-27",
      cost: 1,
      tokens: 2,
      input: 3,
      output: 4,
      cacheRead: 5,
      cacheWrite: 6,
      messages: 7,
      sessions: 8,
    },
  ],
  models: [],
};

const cached: CachedUsage = {
  preset: "today",
  agents: ["claude", "codex"],
  report,
  savedAt: 1,
};

test("usage cache round-trips through the versioned envelope", () => {
  const raw = serializeCachedUsage(cached);
  assert.equal(JSON.parse(raw).version, BOOT_CACHE_VERSION);
  assert.deepEqual(parseCachedUsage(raw), cached);
});

test("usage cache rejects malformed JSON, a wrong version, and a missing report", () => {
  assert.equal(parseCachedUsage("{"), null);
  assert.equal(parseCachedUsage("null"), null);
  assert.equal(parseCachedUsage(JSON.stringify({ version: BOOT_CACHE_VERSION + 1, data: cached })), null);
  assert.equal(
    parseCachedUsage(JSON.stringify({ version: BOOT_CACHE_VERSION, data: { ...cached, report: undefined } })),
    null,
  );
  assert.equal(parseCachedUsage(null), null);
});

test("cached usage matches only the same preset and agent set", () => {
  assert.equal(cachedUsageMatches(cached, "today", ["codex", "claude"]), true);
  assert.equal(cachedUsageMatches(cached, "7d", ["claude", "codex"]), false);
  assert.equal(cachedUsageMatches(cached, "today", ["claude"]), false);
  assert.equal(cachedUsageMatches(cached, "today", ["claude", "opencode"]), false);
  assert.equal(cachedUsageMatches(null, "today", ["claude", "codex"]), false);
});
