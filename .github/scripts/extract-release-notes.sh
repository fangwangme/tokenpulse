#!/usr/bin/env bash
# Prints the CHANGELOG section for one version, for use as the GitHub Release body.
#
# The curated CHANGELOG entry says what changed and why; `gh --generate-notes`
# only lists merged PR titles. Prefer the former, and let the caller fall back
# when a version has no section yet.
#
# Usage: extract-release-notes.sh <version> [changelog-path]
# Exits non-zero when the version has no section.

set -euo pipefail

version="${1:?Usage: extract-release-notes.sh <version> [changelog-path]}"
changelog="${2:-CHANGELOG.md}"

if [[ ! -f "$changelog" ]]; then
  echo "extract-release-notes: no such file: $changelog" >&2
  exit 1
fi

# Print lines after the matching "## [X.Y.Z]" heading, stopping at the next
# top-level section. `-v` passes the version in so it is never treated as a regex.
section="$(
  awk -v want="$version" '
    /^## \[/ {
      # Heading looks like: ## [0.5.0] - 2026-08-15
      inside = ($0 ~ "^## \\[" want "\\]")
      next
    }
    inside { print }
  ' "$changelog"
)"

# Trim leading and trailing blank lines.
section="$(printf '%s\n' "$section" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}')"

if [[ -z "$section" ]]; then
  echo "extract-release-notes: no section for version $version in $changelog" >&2
  exit 1
fi

printf '%s\n' "$section"
