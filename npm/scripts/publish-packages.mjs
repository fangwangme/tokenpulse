#!/usr/bin/env node
// Publishes the packages assembled by build-packages.mjs, in dependency order.
//
// Re-runnable on purpose: publishing five packages is five chances to fail
// halfway, and npm rejects a re-publish of an existing version. Anything already
// on the registry at this version is skipped, so a rerun finishes the release
// instead of erroring out on the packages that did land.

import { execFileSync, spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

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

let published = 0;
let skipped = 0;

for (const dir of manifest.publishOrder) {
  const pkg = JSON.parse(fs.readFileSync(path.join(dir, "package.json"), "utf8"));

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

console.log(`\n${published} published, ${skipped} skipped.`);
