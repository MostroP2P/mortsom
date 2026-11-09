# Pull request gate contract

What a repository's end-to-end gate (MostroP2P/app's Mortsom gate first)
may rely on. Changing anything here is a contract change: bump the schema
or the field, update this document and tell the gate owners in the same
change.

## Scenario catalog: `mortsom list --json`

A gate commits this document and selects scenarios from it, so selection
never depends on a running Mortsom.

```json
{
  "schema": 1,
  "mortsom_version": "0.2.0",
  "scenarios": [
    {
      "name": "happy_sell",
      "family": "happy-path",
      "tags": ["happy-path", "smoke", "parity"],
      "moves_value": true,
      "needs_solver": false,
      "needs_app_wallet": false,
      "needs_bonded_daemon": false,
      "needs": [],
      "requires": ["onboarding", "create-order"],
      "timeout_secs": 1800
    }
  ]
}
```

| Field | Meaning |
|---|---|
| `schema` | Document version, `1`. Bumped on a removed or renamed field or a changed meaning; new fields do not bump it |
| `mortsom_version` | Version of the Mortsom that printed it |
| `name` | Argument `mortsom run` takes |
| `family` | Family tag (spec 8.1) |
| `tags` | Every tag; `mortsom run --tag <tag>` selects by any of them |
| `moves_value` | Moves sats (worthless on regtest) |
| `needs_solver` | Needs a dispute solver registered with the daemon; the regtest stack registers one |
| `needs_app_wallet` | Drives the app's own connected wallet; a run whose payer is a node Mortsom holds skips it |
| `needs_bonded_daemon` | Written for `stack up --bond`; every scenario with `false` is skipped on a bonded daemon, so a gate runs the two groups on separate stacks |
| `needs` | Interruptions outside the app (`background`, `connectivity`, `rotation`, `relay-faults`); Linux and Web runs skip these scenarios |
| `requires` | Capabilities the app profile must support |
| `timeout_secs` | Deadline that aborts one attempt, not its expected duration |

## Results: `artifacts/<run-id>/summary.json`

One run per scenario. A gate reads `results[]` of every summary:

| Field | Meaning |
|---|---|
| `results[].name` | Scenario name |
| `results[].outcome.outcome` | `passed`, `failed` or `skipped` |
| `results[].outcome.category` | For `failed`: `product`, `automation-contract`, `test-infrastructure`, `relay`, `wallet` or `environment-misconfiguration` (spec 10) |
| `results[].outcome.message` | For `failed`: redacted detail |
| `results[].outcome.reason` | For `skipped`: why |
| `results[].attempts` | Attempts made; `2` means the one infrastructure retry was used |
| `results[].duration.secs` | Wall-clock seconds of the last attempt |

Only `product` and `automation-contract` failures are the app's; the
other categories are the harness, the stack or the runner, and a gate
reports them as inconclusive, never as the pull request's regression.

## Running it: `.github/workflows/app-e2e.yml`

A reusable workflow (`workflow_call`) with inputs `app_ref` (branch or
pull request URL), `scenarios` (space-separated names) or `tag`, `bond`
(bonded daemon), `mortsom_ref` (the Mortsom revision to build) and
`artifact_name`. It builds the app from `app_ref` on a GitHub-hosted
runner, runs the selection on Linux against a fresh regtest stack and
uploads the summaries as `<artifact_name>` (one `<run-id>.json` each) and
the sanitized evidence as `<artifact_name>-evidence`. It holds no secret
and a read-only token, and exits non-zero when a scenario fails.
