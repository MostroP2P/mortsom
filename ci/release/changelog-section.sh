#!/usr/bin/env bash
# Print the CHANGELOG.md entry for one version, without its heading.
#
# Usage: ci/release/changelog-section.sh <version>
# Exits non-zero when the version has no section, so callers can tell an
# unreleased commit from a release commit.
set -euo pipefail

version="${1:?usage: changelog-section.sh <version>}"
root="$(git rev-parse --show-toplevel)"
changelog="${root}/CHANGELOG.md"

if [ ! -f "${changelog}" ]; then
  echo "no CHANGELOG.md at ${changelog}" >&2
  exit 1
fi

section="$(
  awk -v version="${version}" '
    index($0, "## [" version "]") == 1 { inside = 1; next }
    inside && index($0, "## [") == 1 { exit }
    inside { print }
  ' "${changelog}"
)"

# Drop the blank line the heading leaves behind; the command substitution
# above already dropped the trailing ones.
section="$(printf '%s\n' "${section}" | sed '/./,$!d')"

if [ -z "${section}" ]; then
  echo "CHANGELOG.md has no entry for ${version}" >&2
  exit 1
fi

printf '%s\n' "${section}"
