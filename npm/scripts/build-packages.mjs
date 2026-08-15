#!/usr/bin/env node
// Assembles the npm packages for a release from the binaries built by CI.
//
// Layout produced under --out:
//   <out>/@fangwangme/tokenpulse-<platform>-<arch>/   one per target, holds the binary
//   <out>/launcher/                                   what users actually install
//
// Publish the platform packages first: the launcher pins them as
// optionalDependencies, and npm resolves those at install time.

import { execFileSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const LAUNCHER_SRC = path.join(HERE, "..", "launcher");
const REPO_ROOT = path.join(HERE, "..", "..");

/** Rust target triple → npm platform package. Keep in sync with release.yml. */
const TARGETS = [
  { triple: "aarch64-apple-darwin", os: "darwin", cpu: "arm64" },
  { triple: "x86_64-apple-darwin", os: "darwin", cpu: "x64" },
  { triple: "aarch64-unknown-linux-gnu", os: "linux", cpu: "arm64" },
  { triple: "x86_64-unknown-linux-gnu", os: "linux", cpu: "x64" },
];

function parseArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i += 2) {
    const key = argv[i]?.replace(/^--/, "");
    if (!key || argv[i + 1] === undefined) {
      throw new Error(`Expected --key value pairs, got: ${argv.join(" ")}`);
    }
    args[key] = argv[i + 1];
  }
  for (const required of ["version", "artifacts", "out"]) {
    if (!args[required]) {
      throw new Error(`Missing --${required}`);
    }
  }
  if (!/^\d+\.\d+\.\d+/.test(args.version)) {
    throw new Error(`--version must look like X.Y.Z, got "${args.version}"`);
  }
  return args;
}

/** The crate version is the source of truth; a mismatch means a mistagged release. */
function assertVersionMatchesCrate(version) {
  const manifest = fs.readFileSync(path.join(REPO_ROOT, "Cargo.toml"), "utf8");
  const crateVersion = manifest.match(/^version\s*=\s*"([^"]+)"/m)?.[1];
  if (crateVersion !== version) {
    throw new Error(
      `Version mismatch: tag says ${version}, Cargo.toml says ${crateVersion}. ` +
        `Bump the crate version or retag.`,
    );
  }
}

/**
 * CI uploads one artifact directory per target, each holding a tarball. Accept
 * either the extracted binary or the tarball so the script also works against a
 * locally assembled directory.
 */
function binaryFor(artifactsDir, triple) {
  const artifactName = `tokenpulse-${triple}`;
  const direct = path.join(artifactsDir, artifactName, "tokenpulse");
  if (fs.existsSync(direct)) return direct;

  const tarball = path.join(artifactsDir, artifactName, `${artifactName}.tar.gz`);
  if (!fs.existsSync(tarball)) {
    throw new Error(`No binary or tarball for ${triple} under ${artifactsDir}`);
  }
  const extractDir = path.join(artifactsDir, artifactName);
  execFileSync("tar", ["-xzf", tarball, "-C", extractDir]);
  if (!fs.existsSync(direct)) {
    throw new Error(`${tarball} did not contain a "tokenpulse" binary`);
  }
  return direct;
}

function writeJson(file, value) {
  fs.mkdirSync(path.dirname(file), { recursive: true });
  fs.writeFileSync(file, `${JSON.stringify(value, null, 2)}\n`);
}

function buildPlatformPackage({ target, version, artifactsDir, outDir }) {
  const name = `@fangwangme/tokenpulse-${target.os}-${target.cpu}`;
  const dir = path.join(outDir, name.replace("/", "__"));
  const binary = binaryFor(artifactsDir, target.triple);

  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });
  const dest = path.join(dir, "bin", "tokenpulse");
  fs.copyFileSync(binary, dest);
  fs.chmodSync(dest, 0o755);

  writeJson(path.join(dir, "package.json"), {
    name,
    version,
    description: `TokenPulse binary for ${target.os} ${target.cpu}`,
    os: [target.os],
    cpu: [target.cpu],
    files: ["bin"],
    repository: {
      type: "git",
      url: "git+https://github.com/fangwangme/tokenpulse.git",
    },
    license: "MIT",
  });

  return { name, dir };
}

function buildLauncher({ version, outDir, platformNames }) {
  const dir = path.join(outDir, "launcher");
  fs.mkdirSync(path.join(dir, "bin"), { recursive: true });

  fs.copyFileSync(
    path.join(LAUNCHER_SRC, "bin", "tokenpulse.js"),
    path.join(dir, "bin", "tokenpulse.js"),
  );
  fs.chmodSync(path.join(dir, "bin", "tokenpulse.js"), 0o755);

  // The repo README is the package page on npmjs.com.
  fs.copyFileSync(path.join(REPO_ROOT, "README.md"), path.join(dir, "README.md"));

  const template = JSON.parse(
    fs.readFileSync(path.join(LAUNCHER_SRC, "package.json"), "utf8"),
  );
  template.version = version;
  template.optionalDependencies = Object.fromEntries(
    platformNames.map((name) => [name, version]),
  );

  // A platform package that exists but is not pinned would never be installed.
  const declared = Object.keys(
    JSON.parse(fs.readFileSync(path.join(LAUNCHER_SRC, "package.json"), "utf8"))
      .optionalDependencies ?? {},
  );
  const missing = declared.filter((name) => !platformNames.includes(name));
  if (missing.length > 0) {
    throw new Error(
      `launcher/package.json pins ${missing.join(", ")}, but no such package was built. ` +
        `Update the template or the target list.`,
    );
  }

  writeJson(path.join(dir, "package.json"), template);
  return { name: template.name, dir };
}

function main() {
  const args = parseArgs(process.argv.slice(2));
  assertVersionMatchesCrate(args.version);

  const outDir = path.resolve(args.out);
  fs.rmSync(outDir, { recursive: true, force: true });
  fs.mkdirSync(outDir, { recursive: true });

  const platforms = TARGETS.map((target) =>
    buildPlatformPackage({
      target,
      version: args.version,
      artifactsDir: path.resolve(args.artifacts),
      outDir,
    }),
  );

  const launcher = buildLauncher({
    version: args.version,
    outDir,
    platformNames: platforms.map((p) => p.name),
  });

  // Publish order matters, so report it explicitly.
  const manifest = {
    version: args.version,
    publishOrder: [...platforms.map((p) => p.dir), launcher.dir],
  };
  writeJson(path.join(outDir, "packages.json"), manifest);

  for (const { name, dir } of [...platforms, launcher]) {
    console.log(`${name} -> ${dir}`);
  }
}

try {
  main();
} catch (error) {
  // A stack trace here is noise: every throw above is a deliberate, actionable
  // check, and this runs in a CI log.
  console.error(`build-packages: ${error.message}`);
  process.exit(1);
}
