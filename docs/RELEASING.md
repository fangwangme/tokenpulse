# Releasing TokenPulse

Maintainer guide for cutting a release, tagging, publishing GitHub binaries, and
distributing the CLI through npm. This is the single source of truth for the
release process — keep it in sync with `.github/workflows/release.yml`.

---

## 1. Versioning

- The workspace uses a single version in the root `Cargo.toml`
  (`[workspace.package] version`); both `tokenpulse-core` and `tokenpulse-cli`
  inherit it via `version.workspace = true`.
- Follow [SemVer](https://semver.org/): bump **patch** for fixes, **minor** for
  backward-compatible features, **major** for breaking changes.
- The config schema has its own `version` integer (`tokenpulse-core/src/config`);
  bump it and add a migration when the on-disk config layout changes.

## 2. Cut a release

1. Make sure `main` is green (`cargo fmt --all -- --check`, `cargo test --workspace --locked`, `cargo build --release`).
2. Bump the version in the root `Cargo.toml`.
3. Move the `CHANGELOG.md` `## [Unreleased]`/in-flight notes into a dated
   `## [X.Y.Z] - YYYY-MM-DD` section.
4. Refresh the lockfile: `cargo build --workspace` (so `Cargo.lock` records the
   new version; CI builds with `--locked`).
5. Commit (e.g. `chore: release vX.Y.Z`) and merge to `main` via PR.
6. Tag and push:
   ```bash
   git tag vX.Y.Z
   git push origin vX.Y.Z
   ```
7. The **Release** workflow (`.github/workflows/release.yml`) triggers on the
   `v*` tag: it builds the release binaries, packages `*.tar.gz` archives, and
   publishes a GitHub Release with auto-generated notes.

> Current `release.yml` builds macOS only (`x86_64-apple-darwin`,
> `aarch64-apple-darwin`). Add Linux (`x86_64-unknown-linux-gnu`,
> `aarch64-unknown-linux-gnu`) and Windows (`x86_64-pc-windows-msvc`) targets to
> the build matrix before advertising those platforms on npm (see §3).

## 3. Publishing to npm

TokenPulse is a compiled Rust binary, so the npm package ships **prebuilt
binaries** rather than source. Use the **per-platform `optionalDependencies`**
pattern (the approach used by esbuild, SWC, and Biome): a thin launcher package
depends on one binary package per platform, and npm installs only the package
matching the user's `os`/`cpu`.

### 3a. Package layout

```
tokenpulse/                      # launcher package (what users install)
  package.json                   # bin + optionalDependencies, version = crate version
  bin/tokenpulse.js              # resolves & execs the platform binary

@tokenpulse/cli-darwin-arm64/    # one package per target
  package.json                   # "os": ["darwin"], "cpu": ["arm64"]
  bin/tokenpulse                 # the prebuilt binary
@tokenpulse/cli-darwin-x64/
@tokenpulse/cli-linux-x64/
@tokenpulse/cli-linux-arm64/
@tokenpulse/cli-win32-x64/       # bin/tokenpulse.exe
```

Launcher `package.json` (key fields):

```json
{
  "name": "tokenpulse",
  "version": "X.Y.Z",
  "bin": { "tokenpulse": "bin/tokenpulse.js" },
  "optionalDependencies": {
    "@tokenpulse/cli-darwin-arm64": "X.Y.Z",
    "@tokenpulse/cli-darwin-x64": "X.Y.Z",
    "@tokenpulse/cli-linux-x64": "X.Y.Z",
    "@tokenpulse/cli-linux-arm64": "X.Y.Z",
    "@tokenpulse/cli-win32-x64": "X.Y.Z"
  }
}
```

Platform `package.json` (npm skips it on non-matching machines):

```json
{
  "name": "@tokenpulse/cli-darwin-arm64",
  "version": "X.Y.Z",
  "os": ["darwin"],
  "cpu": ["arm64"],
  "files": ["bin/tokenpulse"]
}
```

Launcher `bin/tokenpulse.js`:

```js
#!/usr/bin/env node
const { spawnSync } = require("node:child_process");
const pkg = `@tokenpulse/cli-${process.platform}-${process.arch}`;
const exe = process.platform === "win32" ? "tokenpulse.exe" : "tokenpulse";
let binary;
try {
  binary = require.resolve(`${pkg}/bin/${exe}`);
} catch {
  console.error(`tokenpulse: no prebuilt binary for ${process.platform}-${process.arch}`);
  process.exit(1);
}
process.exit(spawnSync(binary, process.argv.slice(2), { stdio: "inherit" }).status ?? 1);
```

All package versions must equal the crate version so the launcher's pinned
`optionalDependencies` resolve.

### 3b. Publish steps

1. Download the release archives built by `release.yml` (or rebuild locally per
   target with `cargo build --release -p tokenpulse-cli --target <triple>`).
2. For each target, assemble `@tokenpulse/cli-<platform>-<arch>` with the binary
   and a generated `package.json`, then `npm publish --access public`.
3. Publish the launcher `tokenpulse` package last (so its
   `optionalDependencies` already exist): `npm publish --access public`.
4. Smoke-test in a clean dir: `npx tokenpulse@X.Y.Z --help`.

### 3c. Turnkey alternative — `dist` (cargo-dist)

[`dist`](https://opensource.axo.dev/cargo-dist/) automates all of the above. It
builds every target in CI, creates the GitHub Release, and can emit an npm
installer package directly:

```bash
cargo install cargo-dist
dist init                       # choose targets + "npm" installer
# commit the generated CI + Cargo.toml [workspace.metadata.dist] config
git tag vX.Y.Z && git push origin vX.Y.Z   # dist's CI builds + publishes
```

Set `NPM_TOKEN` as a CI secret for automated `npm publish`. This is the
recommended path if we want to support npm long-term, since it keeps the
per-platform packages and versions in sync automatically.

## 4. After release

- Verify the GitHub Release lists every expected archive.
- Verify `npx tokenpulse` / `npm i -g tokenpulse` runs the new version.
- Open the next `## [Unreleased]` CHANGELOG section for ongoing work.
