#!/usr/bin/env node
// Publishes the packages assembled by build-packages.mjs, in dependency order.
//
// Publishes the platform binary packages first, then polls the npm registry until
// all platform binaries are available before publishing the launcher package.
// This prevents npm's asynchronous processing queue from returning 404 for optional
// binary dependencies when users immediately install or run the new release.
//
// Re-runnable on purpose: publishing five packages is five chances to fail
// halfway, and npm rejects a re-publish of an existing version. Anything already
// on the registry at this version is skipped, so a rerun finishes the release
// instead of erroring out on the packages that did land.

import { execFile, execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { setTimeout as sleep } from "node:timers/promises";
import { promisify } from "node:util";

const execFileAsync = promisify(execFile);

const DEFAULT_POLL_INTERVAL_MS = 10_000;
const DEFAULT_TIMEOUT_MS = 5 * 60 * 1000; // 5 minutes

const pollIntervalMs =
  Number(process.env.TOKENPULSE_PUBLISH_POLL_INTERVAL_MS) || DEFAULT_POLL_INTERVAL_MS;
const timeoutMs =
  Number(process.env.TOKENPULSE_PUBLISH_TIMEOUT_MS) || DEFAULT_TIMEOUT_MS;

function fail(message) {
  console.error(`publish-packages: ${message}`);
  process.exit(1);
}

const outDir = process.argv[2];
if (!outDir) {
  console.error("Usage: publish-packages.mjs <dist-dir> [--dry-run]");
  process.exit(1);
}
const dryRun = process.argv.includes("--dry-run");

let manifest;
try {
  manifest = JSON.parse(fs.readFileSync(path.join(outDir, "packages.json"), "utf8"));
} catch (error) {
  fail(`cannot read ${path.join(outDir, "packages.json")}: ${error.message}`);
}

function alreadyPublished(name, version) {
  const result = spawnSync("npm", ["view", `${name}@${version}`, "version"], {
    encoding: "utf8",
  });
  // A missing package or version exits non-zero (E404); anything else printed
  // means this exact version is on the registry.
  return result.status === 0 && result.stdout.trim() === version;
}

async function checkPackageReady(name, version) {
  try {
    const { stdout } = await execFileAsync("npm", ["view", `${name}@${version}`, "version"]);
    return stdout.trim() === version;
  } catch {
    return false;
  }
}

async function waitForPlatformPackagesReady(packages, { timeoutMs, pollIntervalMs }) {
  if (packages.length === 0) return;

  let pending = [...packages];
  const startTime = Date.now();
  console.log(
    `\nVerifying registry readiness for ${pending.length} platform package(s) before publishing launcher...`,
  );

  while (pending.length > 0) {
    const elapsedMs = Date.now() - startTime;
    if (elapsedMs >= timeoutMs) {
      const remaining = pending.map((p) => `${p.name}@${p.version}`).join(", ");
      fail(
        `timed out after ${Math.round(timeoutMs / 1000)}s waiting for platform package(s) on npm registry: ${remaining}. ` +
          `npm async processing queue may be delayed. Rerun this workflow once packages are processed.`,
      );
    }

    const results = await Promise.all(
      pending.map(async (pkg) => ({
        pkg,
        ready: await checkPackageReady(pkg.name, pkg.version),
      })),
    );

    const newlyReady = results.filter((r) => r.ready).map((r) => r.pkg);
    for (const pkg of newlyReady) {
      console.log(`✓      ${pkg.name}@${pkg.version} is ready on npm registry`);
    }

    pending = results.filter((r) => !r.ready).map((r) => r.pkg);
    if (pending.length === 0) {
      console.log(`All platform packages are verified ready on npm registry.\n`);
      return;
    }

    const elapsedSec = Math.round(elapsedMs / 1000);
    const timeoutSec = Math.round(timeoutMs / 1000);
    const waitSec = Math.round(pollIntervalMs / 1000);
    console.log(
      `[${elapsedSec}s / ${timeoutSec}s] Waiting for ${pending.length} package(s) ` +
        `(${pending.map((p) => p.name).join(", ")})... retrying in ${waitSec}s`,
    );

    await sleep(pollIntervalMs);
  }
}

const packageEntries = manifest.publishOrder.map((dir) => {
  try {
    const pkg = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));
    return { dir, pkg };
  } catch (error) {
    fail(`cannot read ${path.join(dir, "package.json")}: ${error.message}`);
  }
});

const platformPackages = packageEntries.filter((p) => p.pkg.name !== "@fangwangme/tokenpulse");
const launcherEntry = packageEntries.find((p) => p.pkg.name === "@fangwangme/tokenpulse");

let published = 0;
let skipped = 0;

for (const { dir, pkg } of platformPackages) {
  if (alreadyPublished(pkg.name, pkg.version)) {
    console.log(`skip   ${pkg.name}@${pkg.version} (already published)`);
    skipped += 1;
    continue;
  }

  const args = ["publish", "--access", "public"];
  if (dryRun) args.push("--dry-run");

  console.log(`publish ${pkg.name}@${pkg.version}${dryRun ? " (dry run)" : ""}`);
  try {
    execFileSync("npm", args, { cwd: dir, stdio: "inherit" });
  } catch {
    fail(
      `npm publish failed for ${pkg.name}@${pkg.version}. ` +
        `Packages published before it stay published; rerun to finish the rest.`,
    );
  }
  published += 1;
}

if (launcherEntry) {
  const { dir, pkg } = launcherEntry;

  if (alreadyPublished(pkg.name, pkg.version)) {
    console.log(`skip   ${pkg.name}@${pkg.version} (already published)`);
    skipped += 1;
  } else {
    const depsToVerify = Object.entries(pkg.optionalDependencies ?? {}).map(
      ([name, version]) => ({ name, version }),
    );

    if (dryRun && process.env.TOKENPULSE_FORCE_VERIFY_READY !== "1") {
      console.log(
        `dry run: skipping registry readiness wait for platform packages (${depsToVerify.length} packages)\n`,
      );
    } else if (depsToVerify.length > 0) {
      await waitForPlatformPackagesReady(depsToVerify, {
        timeoutMs,
        pollIntervalMs,
      });
    }

    const args = ["publish", "--access", "public"];
    if (dryRun) args.push("--dry-run");

    console.log(`publish ${pkg.name}@${pkg.version}${dryRun ? " (dry run)" : ""}`);
    try {
      execFileSync("npm", args, { cwd: dir, stdio: "inherit" });
    } catch {
      fail(
        `npm publish failed for ${pkg.name}@${pkg.version}. ` +
          `Packages published before it stay published; rerun to finish the rest.`,
      );
    }
    published += 1;
  }
}

console.log(`\n${published} published, ${skipped} skipped.`);
