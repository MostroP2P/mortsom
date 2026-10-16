#!/usr/bin/env bash
# Block until the CI workflow run for a commit has finished, and fail unless
# it succeeded. Used by the publish job of the Release workflow so that a
# release commit is never tagged on a red build.
#
# Usage: ci/release/wait-for-ci.sh <sha>
# Needs: gh with GH_TOKEN, GITHUB_REPOSITORY (set by GitHub Actions).
set -euo pipefail

sha="${1:?usage: wait-for-ci.sh <sha>}"
workflow="${CI_WORKFLOW:-ci.yml}"
poll_seconds="${CI_POLL_SECONDS:-30}"
timeout_seconds="${CI_TIMEOUT_SECONDS:-3600}"
deadline=$(( $(date +%s) + timeout_seconds ))

while :; do
  run="$(gh run list --workflow "${workflow}" --commit "${sha}" \
    --json databaseId,status,conclusion --jq '.[0] // empty')"
  if [ -n "${run}" ]; then
    status="$(jq -r .status <<<"${run}")"
    if [ "${status}" = "completed" ]; then
      conclusion="$(jq -r .conclusion <<<"${run}")"
      run_id="$(jq -r .databaseId <<<"${run}")"
      if [ "${conclusion}" = "success" ]; then
        echo "CI run ${run_id} for ${sha} succeeded"
        exit 0
      fi
      echo "CI run ${run_id} for ${sha} finished with '${conclusion}'; not publishing" >&2
      exit 1
    fi
    echo "CI run for ${sha} is ${status}; waiting"
  else
    echo "no CI run for ${sha} yet; waiting"
  fi
  if [ "$(date +%s)" -ge "${deadline}" ]; then
    echo "CI for ${sha} did not finish within ${timeout_seconds}s; not publishing" >&2
    exit 1
  fi
  sleep "${poll_seconds}"
done
