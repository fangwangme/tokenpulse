# Releasing TokenPulse

Maintainer guide for cutting a release, tagging, publishing GitHub binaries, and
distributing the CLI through npm. This is the single source of truth for the
release process — keep it in sync with `.github/workflows/release.yml`.

---

## How a release works

A release happens in two phases, and the split matters:

| Phase | Where | What |
| --- | --- | --- |
| **A. Release PR** | a branch, reviewed and merged like any other change | version bump, CHANGELOG, verification |
| **B. Tag** | on `main`, after the merge | `git tag` + push; CI does the rest |

Nothing is ever committed straight to `main` — the release commit goes through
a PR the same as a feature does. The tag in phase B is the one thing done on
`main` directly, and a tag adds no commit: it just points at what was merged.

Two version numbers exist and they are unrelated. The **release** version lives
once in the root `Cargo.toml` and is what this document is about. The **config
schema** version is an integer in `tokenpulse-core/src/config`, bumped with a
migration whenever the on-disk config layout changes — independently of any
release. The database schemas in `history/` and `usage/store.rs` are versioned
the same way, via `PRAGMA user_version`.

---

## 1. Phase A — the release PR

Everything in this phase happens on a branch. Per the project convention, work
in a worktree rather than switching `main` in place:

```bash
git switch main && git pull
mkdir -p .worktrees
git worktree add .worktrees/release-vX.Y.Z -b chore/release-vX.Y.Z
cd .worktrees/release-vX.Y.Z
```

### Step 1 — Decide the version

Look at what landed since the last tag and pick per [SemVer](https://semver.org/):

```bash
git log --oneline "$(git describe --tags --abbrev=0)"..main
```

- **patch** (`0.5.0` → `0.5.1`) — fixes only, no new behaviour.
- **minor** (`0.5.0` → `0.6.0`) — new features, existing setups keep working.
- **major** — anything that breaks an existing config, database, or command.

### Step 2 — Write the release note

`CHANGELOG.md` **is** the release note: CI publishes that section verbatim as
the GitHub Release body. Nothing else needs writing.

Ideally each feature PR already added its lines under `## [Unreleased]` as it
merged — creating that heading if it was not there — and this step is just
renaming it to `## [X.Y.Z] - YYYY-MM-DD` and tidying it. When entries are
missing, write them now from the commit log. A release leaves no empty
`## [Unreleased]` behind.

```markdown
## [X.Y.Z] - YYYY-MM-DD

### Added
- **Short bold label**: what it does, and why it exists.

### Changed
- What behaves differently now, and what a user has to do about it.

### Fixed
- What was broken, what the symptom was, and what fixes it.
```

Write for someone deciding whether to upgrade. Name the user-visible symptom,
not the internal function. Call out anything that changes existing behaviour,
needs a manual step, or alters a default — those are the lines people actually
need. Use `Added` / `Changed` / `Fixed` / `Removed` and omit empty ones.

### Step 3 — Bump the version

Only one place holds it; both crates inherit from the workspace:

```bash
# edit [workspace.package] version in the root Cargo.toml
cargo build --workspace          # refreshes Cargo.lock — CI builds with --locked
```

Commit `Cargo.lock` too. CI builds with `--locked` and fails if it is stale.

### Step 4 — Verify locally

```bash
cargo fmt --all -- --check
cargo test --workspace --locked
cargo build --release --workspace
```

### Step 5 — Open the PR and merge it

```bash
git commit -am "chore: release vX.Y.Z"
git push -u origin chore/release-vX.Y.Z
gh pr create --fill
```

Wait for CI to pass, then merge. Do not tag yet — the tag has to point at the
merge commit on `main`, not at the branch.

```bash
gh pr merge --squash --delete-branch
```

---

## 2. Phase B — tag the merged commit

```bash
cd <repo root> && git switch main && git pull
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag must be `vX.Y.Z` and must match the version in `Cargo.toml`, or the npm
job stops before publishing anything.

### Step 6 — CI takes over

`.github/workflows/release.yml` fires on the `v*` tag:

| Job | Does |
| --- | --- |
| `build` | One release binary per target, uploaded as an artifact |
| `publish-release` | GitHub Release with the `*.tar.gz` archives, body taken from the CHANGELOG section |
| `publish-npm` | Assembles and publishes the five npm packages (§3) |

Targets: `x86_64-apple-darwin`, `aarch64-apple-darwin`,
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`. Linux builds run on
`ubuntu-22.04`, so the binaries need only glibc 2.35 and still run on distros a
couple of releases behind.

The Rust target triples are internal build identifiers. GitHub Release assets
use user-facing names instead:

| Rust target | Release asset |
| --- | --- |
| `x86_64-apple-darwin` | `tokenpulse-darwin-x64.tar.gz` |
| `aarch64-apple-darwin` | `tokenpulse-darwin-arm64.tar.gz` |
| `x86_64-unknown-linux-gnu` | `tokenpulse-linux-x64-gnu.tar.gz` |
| `aarch64-unknown-linux-gnu` | `tokenpulse-linux-arm64-gnu.tar.gz` |

Both macOS targets build on `macos-14`; the x86_64 one is cross-compiled. Do not
reintroduce the Intel `macos-13` runner — GitHub no longer allocates it, and a
job requesting it does not fail fast. It sits queued for the full 24-hour limit
and is then cancelled, taking the whole release with it. Every release before
v0.5.0 was lost this way.

`publish-release` and `publish-npm` both wait on `build`, so a failure on any
platform stops the release before anything is published.

### Step 7 — Verify the result

```bash
gh run watch                                        # follow the release run
gh release view vX.Y.Z                              # archives + notes present?
npx @fangwangme/tokenpulse@X.Y.Z --version          # in a clean directory
```

Then clean up the worktree from phase A:

```bash
git worktree remove .worktrees/release-vX.Y.Z
```

### If something fails

Anything that goes wrong before `publish-*` runs leaves nothing published, so
the fix is always: delete the tag, correct the branch through a normal PR, and
tag again.

```bash
git tag -d vX.Y.Z && git push --delete origin vX.Y.Z
```

| Failure | What to do |
| --- | --- |
| A build job fails | Nothing was published. Fix on a branch, merge, delete the tag, re-tag. |
| `publish-npm` fails on version mismatch | The tag and `Cargo.toml` disagree. Nothing was published — delete the tag and re-tag correctly. |
| `publish-npm` fails midway | Some packages are live. Re-run the job; it skips what already published (§3c). |
| A bad build reached npm | A version can never be replaced. `npm deprecate` it and release a patch. |
| Wrong release notes | Edit the GitHub Release body directly, and fix `CHANGELOG.md` through a PR. |

## 3. npm packaging reference

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

Verification is Step 7 above. Nothing else is left — do **not** open a PR just
to add an empty `## [Unreleased]` heading. The next change to land writes that
heading itself, in the same PR that adds the entry under it.
