#!/usr/bin/env bash
# Print the shared workspace version from the root manifest.
#
# Usage: ci/release/version.sh   (run from the repository root)
set -euo pipefail

root="$(git rev-parse --show-toplevel)"
version="$(
  sed -n '/^\[workspace\.package\]/,/^\[[^w]/p' "${root}/Cargo.toml" |
    sed -n 's/^version *= *"\([^"]*\)".*/\1/p' |
    head -n 1
)"

if [ -z "${version}" ]; then
  echo "no [workspace.package] version in Cargo.toml" >&2
  exit 1
fi

printf '%s\n' "${version}"
