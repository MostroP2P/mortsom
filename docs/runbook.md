# Mortsom runbook

Operational guide for the people who run Mortsom (spec Phase 6.6). It covers
what to do when a run fails, how to recover a host, how secrets are rotated
and how incidents are handled. Every command below is `mortsom …`; nothing
here bypasses the safety switches described in `README.md`.

## 1. Read the failure category first

`summary.json` and the published `publish.json` classify every scenario:

| Category | Meaning | Retry policy |
|---|---|---|
| `product` | the app misbehaved: wrong state, wrong copy, illegal transition, repeated protocol message | never retried; file an app bug with the timeline and screenshots |
| `automation-contract` | a stable id or bootstrap hook the app promised is missing | never retried; fix the app branch (`docs/automation-contract.md` in the mobile repo) or the adapter |
| `test-infrastructure` | emulator, Appium, adb, Docker or host tooling failed | retried once by the runner, unless the attempt already moved sats |
| `relay` | the run relay failed or evidence never arrived | retried once as above; check the container log |
| `wallet` | a dedicated wallet failed, rejected or timed out | never retried; see §4 |
| `environment-misconfiguration` | configuration, secrets or prerequisites wrong or missing | fix and rerun `doctor` |

A run is only *passed* when every scenario passed and cleanup left nothing
open. A `skipped` scenario is a missing capability or precondition; it is
never a pass.

## 2. Infrastructure failures (`test-infrastructure`, `relay`)

1. Open `run.log`, `device-logcat.txt` and `relay-container.log` in the run
   directory (all redacted at write time).
2. Emulator did not boot within `android.boot_timeout_secs`: check `/dev/kvm`
   access, RAM (`android.ram_mb` per emulator, two of them) and free disk
   for snapshots. `mortsom device smoke` reproduces the boot alone.
3. Appium session failed to start: `appium --version` and the UiAutomator2
   driver version must match the pins in `ci/Dockerfile`; a crashed
   instrumentation (`instrumentation process unexpectedly terminated`) is
   an emulator/driver mismatch, not an app bug.
4. Relay: `mortsom relay smoke` proves the container, the port and the
   daemon's use of it. A run relay that dies mid-run shows in
   `relay-container.log`; a relay that answers but never delivers events
   usually means the daemon's `settings.toml` relay list does not include the
   run relay URL for the daemon's host (`mortsom doctor` checks equivalence
   across `127.0.0.1`, `localhost`, `host.docker.internal`, `10.0.2.2`).
5. After fixing, run `mortsom cleanup <run-id>` (§6) before starting again.

## 3. App regressions (`product`, `automation-contract`)

1. `timeline.md` shows the exact step, the state each app displayed and the
   relay/wallet evidence around it; the `<actor>/steps/` screenshots and UI
   hierarchies are named after those steps.
2. A recovery scenario failing with `unexpected protocol message` means the
   app repeated an action after a restart/background: report it with the
   `relay-events.jsonl` sequence numbers.
3. `visible_actions` mismatches (`unauthorized_actions_at_taken`) list what
   was offered versus what the role and state allow.
4. Contract failures are fixed on the app's automation branch
   (`mortsom-automation-contract`), never by loosening the adapter.

## 4. Payment failures (`wallet`)

The runner never retries once sats moved. When a payment scenario fails:

1. `wallet-payments.json` holds the ledger: every reservation, in-flight
   payment (with payment hash) and settlement, plus the run totals.
2. An `in-flight` entry after the run means the wallet did not report a
   terminal state within `run.settle_timeout_secs`. Look the hash up in the
   wallet; a hold invoice that settled later means the trade completed and
   the order must be checked (§6); a failed one needs no action.
3. `wallet unavailable` at bootstrap: NWC relay unreachable or the wallet
   offline; `mortsom doctor` verifies both wallets answer `get_info` and
   `get_balance` before a run.
4. Insufficient liquidity: `doctor` fails the wallet check when the balance
   is below `run.max_run_spend_sats`; top up the dedicated wallet, never
   raise the cap to make a run pass.

## 5. Relay failures — see §2.4; additionally

- Duplicate/stale replay scenarios use `mortsom`'s own replay of signed
  daemon events; a `relay` failure there (`replay: … not connected`) means
  the run relay was unreachable from the host, not an app bug.

## 6. Cleanup recovery

After any interrupted or failed run:

```sh
mortsom cleanup <run-id>
```

It removes that run's relay container, shuts down Mortsom's own emulators
(never a foreign AVD on the same slot) and prints the orders the run's own
sweep left open, taken from `cleanup.json`. It **cannot** cancel orders: for
each `UNRESOLVED order`, open the maker's app (Alice) on a device, cancel
it, and if the order is funded ask the counterpart (Bob) to accept the
cooperative cancel. If `cleanup.json` is missing the run died before its
sweep and the orders are `UNKNOWN`: check both apps' trade lists.

The next run starts with `mortsom doctor`; a clean host shows no
`mortsom-relay-*` container and no `mortsom-*` emulator running.

## 7. Secret rotation

Secrets are only ever `env:` references (`MORTSOM_ALICE_NWC`,
`MORTSOM_BOB_NWC`, `MORTSOM_MOSTRO_SETTINGS_FILE`, `MORTSOM_MAINNET_CONFIRM`).

1. Create the new NWC connection in the wallet; keep the old one until the
   first run with the new one passes `doctor`.
2. Update the GitHub environment secret (`integration` or `canary`), then
   revoke the old connection in the wallet.
3. Rotate immediately if a URI ever appears unredacted anywhere: the
   redactor removes configured values and NWC/nsec/hex/base64 patterns from
   every artifact, and `mortsom publish` refuses a bundle in which a
   configured secret survives — a warning `file(s) still contained
   secret-shaped text` in a publish log is itself a reason to investigate.
4. Daemon key rotation is the daemon operator's; update
   `MORTSOM_MOSTRO_PUBKEY` and the apps' bootstrap picks it up on the next
   run.

## 8. Incident response

1. **Stop the bleeding**: a canary or integration failure creates a
   `canary-failure` issue and does not rerun. Do not dispatch a new run
   until §6 confirms nothing is open and §4 confirms no payment is in
   flight.
2. **Preserve evidence**: the published bundle (redacted) is attached to the
   workflow run (retention: 14 days PR, 30 integration, 90 canary). Raw
   artifacts stay on the self-hosted runner under `MORTSOM_ARTIFACTS_DIR`,
   readable only by maintainers with host access; nothing prunes them
   automatically, so the runner owner deletes run directories older than 14
   days (`find "$MORTSOM_ARTIFACTS_DIR" -maxdepth 1 -mtime +14`) as part of
   host maintenance.
3. **Classify**: product bug → app issue with timeline; infrastructure →
   fix the host or pins; wallet → §4; secrets exposure → §7 and note it in
   the issue.
4. **Accounting**: `mortsom accounting --lane canary` shows settled sats,
   fees, leftovers and pass/fail per run; a run with leftovers must be
   explained in its issue.
5. **Resume**: rerun manually via `workflow_dispatch` once the issue is
   closed with the root cause written down.
