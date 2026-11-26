import assert from "node:assert/strict";
import { test } from "node:test";
import { formatCost } from "../src/shared/money.ts";

// usage-report.md "Numbers": the compact money format shared by every phone surface.
test("compact money boundaries", () => {
  assert.equal(formatCost(999.99), "$999.99");
  assert.equal(formatCost(1000), "$1.000K");
  assert.equal(formatCost(9876.54), "$9.877K");
  assert.equal(formatCost(10000), "$10.000K");
  assert.equal(formatCost(1234567), "$1.235M");
});
