#!/usr/bin/env node
"use strict";

// Resolves the prebuilt binary for this machine and hands the terminal to it.
//
// The launcher package depends on one binary package per platform through
// `optionalDependencies`, and npm installs only the one whose `os`/`cpu` match.
// So exactly one of these resolves, and a missing one means we publish no build
// for this platform rather than an installation being broken.

const { spawnSync } = require("node:child_process");

const platformPackage = `@fangwangme/tokenpulse-${process.platform}-${process.arch}`;
const executable = process.platform === "win32" ? "tokenpulse.exe" : "tokenpulse";

let binary;
try {
  binary = require.resolve(`${platformPackage}/bin/${executable}`);
} catch {
  console.error(
    `tokenpulse: no prebuilt binary for ${process.platform}-${process.arch}.\n` +
      `Install from source instead:\n` +
      `  cargo install --git https://github.com/fangwangme/tokenpulse tokenpulse-cli`,
  );
  process.exit(1);
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
