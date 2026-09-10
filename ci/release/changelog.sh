#!/usr/bin/env bash
# cargo-release pre-release hook: regenerate CHANGELOG.md for the version
# being released.
#
# cargo-release runs the hook once per workspace member, so the work is
# guarded to happen a single time per release: the real run stops as soon as
# the changelog already carries the version, the dry run on a marker file.
set -euo pipefail

version="${NEW_VERSION:?NEW_VERSION is set by cargo-release}"
tag="v${version}"
root="${WORKSPACE_ROOT:-$(git rev-parse --show-toplevel)}"
changelog="${root}/CHANGELOG.md"

if [ "${DRY_RUN:-false}" = "true" ]; then
  # Every hook invocation of one dry run shares cargo-release as its parent,
  # so the pid keeps the notice to one line per run instead of one per crate.
  marker="${root}/target/.release-dry-run-${PPID}"
  mkdir -p "${root}/target"
  find "${root}/target" -maxdepth 1 -name '.release-dry-run-*' -mmin +60 -delete 2>/dev/null || true
  [ -e "${marker}" ] && exit 0
  : > "${marker}"
  echo "release: dry run, CHANGELOG.md left untouched (${tag})"
  exit 0
fi

if [ -f "${changelog}" ] && grep -q "^## \[${version}\]" "${changelog}"; then
  exit 0
fi

if ! command -v git-cliff >/dev/null 2>&1; then
  echo "git-cliff not found; install it with: cargo install git-cliff" >&2
  exit 1
fi

cd "${root}"
git-cliff --config cliff.toml --tag "${tag}" --output "${changelog}"
# cargo-release commits what it bumped plus whatever is already staged;
# without this the release commit leaves CHANGELOG.md untracked.
git add "${changelog}"
echo "release: CHANGELOG.md written for ${tag}"
