#!/usr/bin/env node
"use strict";

// Resolves the prebuilt binary for this machine and hands the terminal to it.
//
// The launcher package depends on one binary package per platform through
// `optionalDependencies`, and npm installs only the one whose `os`/`cpu` match.
// So exactly one of these resolves, and a missing one means we publish no build
// for this platform rather than an installation being broken.

const { spawnSync } = require("node:child_process");
const fs = require("node:fs");
const path = require("node:path");

const platformPackage = `@fangwangme/tokenpulse-${process.platform}-${process.arch}`;
const executable = process.platform === "win32" ? "tokenpulse.exe" : "tokenpulse";

const SUPPORTED_PLATFORMS = new Set([
  "darwin-arm64",
  "darwin-x64",
  "linux-arm64",
  "linux-x64",
]);

function readJsonSafe(filePath) {
  try {
    return JSON.parse(fs.readFileSync(filePath, "utf8"));
  } catch {
    return null;
  }
}

let binary;
try {
  binary = require.resolve(`${platformPackage}/bin/${executable}`);
} catch {
  const currentPlatformKey = `${process.platform}-${process.arch}`;
  if (SUPPORTED_PLATFORMS.has(currentPlatformKey)) {
    console.error(
      `tokenpulse: binary package ${platformPackage} was not installed.\n` +
        `This usually happens when the package manager skips optional dependencies or encounters registry delays.\n` +
        `To reinstall:\n` +
        `  bun add -g --force @fangwangme/tokenpulse\n` +
        `  # or: npm install -g --force @fangwangme/tokenpulse\n` +
        `Or install from source:\n` +
        `  cargo install --git https://github.com/fangwangme/tokenpulse tokenpulse-cli`,
    );
  } else {
    console.error(
      `tokenpulse: no prebuilt binary for ${process.platform}-${process.arch}.\n` +
        `Install from source instead:\n` +
        `  cargo install --git https://github.com/fangwangme/tokenpulse tokenpulse-cli`,
    );
  }
  process.exit(1);
}

// Check version consistency between launcher shell and platform binary package.
try {
  const launcherPkg = readJsonSafe(path.join(__dirname, "..", "package.json"));
  let platformPkg = readJsonSafe(path.join(path.dirname(binary), "..", "package.json"));
  if (!platformPkg) {
    try {
      platformPkg = readJsonSafe(require.resolve(`${platformPackage}/package.json`));
    } catch {
      // ignore
    }
  }

  if (
    launcherPkg &&
    platformPkg &&
    launcherPkg.version &&
    platformPkg.version &&
    launcherPkg.version !== platformPkg.version
  ) {
    console.warn(
      `tokenpulse: warning: version mismatch detected.\n` +
        `  launcher shell: v${launcherPkg.version}\n` +
        `  binary package: v${platformPkg.version} (${platformPackage})\n` +
        `This can occur if optional dependencies were cached or skipped during update.\n` +
        `To update the binary package, run:\n` +
        `  bun add -g --force @fangwangme/tokenpulse@${launcherPkg.version}\n` +
        `  # or: npm install -g --force @fangwangme/tokenpulse@${launcherPkg.version}\n`,
    );
  }
} catch {
  // Best-effort check; do not block execution if package inspection fails.
}

// `inherit` matters: the dashboard is a full-screen TUI and needs the real tty
// for raw mode, mouse capture, and size detection.
const result = spawnSync(binary, process.argv.slice(2), { stdio: "inherit" });

if (result.error) {
  console.error(`tokenpulse: failed to launch ${binary}: ${result.error.message}`);
  process.exit(1);
}

// A process killed by a signal reports a null status; report it the way a shell
// would so callers can tell an interrupt from a clean non-zero exit.
if (result.signal) {
  process.exit(128 + (require("node:os").constants.signals[result.signal] ?? 0));
}

process.exit(result.status ?? 1);
