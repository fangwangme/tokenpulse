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
   `v*` tag. It builds one binary per target, publishes a GitHub Release with
   the `*.tar.gz` archives and auto-generated notes, and publishes the npm
   packages (see §3).

Targets built: `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. Linux builds run on
`ubuntu-22.04` so the binaries need only glibc 2.35 and still run on distros a
couple of releases behind.

## 3. Publishing to npm

Automated by `release.yml`; the `v*` tag that builds the binaries also publishes
them. Nothing here needs running by hand.

The plain name `tokenpulse` is taken on npm by an unrelated package, so
everything ships under the `@fangwangme` scope.

### 3a. Package layout

TokenPulse is a compiled binary, so npm ships **prebuilt binaries** using the
per-platform `optionalDependencies` pattern (as esbuild, SWC, and Biome do): a
thin launcher depends on one binary package per platform, and npm installs only
the one matching the user's `os`/`cpu`.

```
@fangwangme/tokenpulse                  launcher — what users install
  bin/tokenpulse.js                     resolves and execs the platform binary
@fangwangme/tokenpulse-darwin-arm64     one per target, holds the binary
@fangwangme/tokenpulse-darwin-x64
@fangwangme/tokenpulse-linux-arm64
@fangwangme/tokenpulse-linux-x64
```

Sources live in `npm/`:

| Path | Role |
| --- | --- |
| `npm/launcher/` | Launcher `package.json` template and `bin/tokenpulse.js`. The template's `optionalDependencies` are the authoritative list of platforms we claim to support. |
| `npm/scripts/build-packages.mjs` | Turns the CI build artifacts into ready-to-publish package directories. |
| `npm/scripts/publish-packages.mjs` | Publishes them in dependency order. |

All package versions equal the crate version, so the launcher's pinned
`optionalDependencies` resolve. `build-packages.mjs` refuses to run when the tag
and `Cargo.toml` disagree — check that first if the npm job fails immediately.

### 3b. Adding a platform

Three places have to agree, and the build fails loudly if they do not:

1. `release.yml` — add a `runner` / `target` pair to the `build` matrix.
2. `npm/scripts/build-packages.mjs` — add the triple to `TARGETS`.
3. `npm/launcher/package.json` — add the package to `optionalDependencies`.

Windows is deliberately absent: the code has `cfg(windows)` branches but has
never been built or exercised there, so it is not advertised on npm.

### 3c. Recovering a partial publish

`publish-packages.mjs` skips any package already on the registry at this
version, so re-running the failed job finishes the release. A published version
can never be replaced — if a bad build reached the registry, `npm deprecate` it
and cut a new patch version.

To rehearse the whole thing without touching the registry:

```bash
node npm/scripts/build-packages.mjs --version 0.5.0 --artifacts dist --out .local/npm-dist
node npm/scripts/publish-packages.mjs .local/npm-dist --dry-run
```

### 3d. Prerequisites

`NPM_TOKEN` must exist as a repository secret, from an npm automation token with
publish rights on the `@fangwangme` scope. Scoped packages default to private,
so the scripts pass `--access public`.

## 4. After release

- Verify the GitHub Release lists every expected archive.
- Verify the npm publish landed: `npx @fangwangme/tokenpulse@X.Y.Z --version`
  in a clean directory, ideally on both macOS and Linux.
- Open the next `## [Unreleased]` CHANGELOG section for ongoing work.
