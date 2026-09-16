# Mortsom runbook

What to do when a run fails, how to recover, and how secrets are handled. Every command is `mortsom …` from
the repository root. Nothing here bypasses the value-safety rules in [spec.md](../spec.md) section 10.

## 1. Read the failure category first

`summary.json` classifies every scenario. The category is assigned by typed errors, never inferred from text.

| Category | Meaning | What to do |
|---|---|---|
| `product` | The app misbehaved: wrong state, wrong copy, illegal transition | Never retried. File an app issue with the timeline and screenshots |
| `automation-contract` | A stable identifier or bootstrap hook the app promised is missing | Never retried. Fix the app's `docs/automation-contract.md` branch or the adapter; never loosen the adapter to pass |
| `test-infrastructure` | Xvfb, D-Bus, Secret Service, ChromeDriver, browser, Docker or host tooling failed | No automatic retry. Fix the host, then `cleanup`, then a fresh run |
| `relay` | The relay failed or expected evidence never arrived | No automatic retry. Check `stack status`, `stack logs relay` and `stack logs` (the daemon) |
| `wallet` | An LND wallet failed, rejected or timed out | Never retried. See section 4 |
| `environment-misconfiguration` | Configuration, secrets or prerequisites wrong or missing | Fix and rerun `doctor` |

A run passes only when the scenario passed and cleanup left nothing open. A skipped scenario is never a pass.

## 2. Infrastructure and relay failures

1. Read `run.log` in the run directory. It is redacted at write time.
2. Linux: confirm Xvfb, D-Bus, GNOME Keyring and the AT-SPI registry are installed and that no other
   process holds the actor's display. `doctor --platform linux` names missing tools.
3. Web: confirm the browser and ChromeDriver versions match. Override locations with `MORTSOM_BROWSER` and
   `MORTSOM_CHROMEDRIVER`.
4. Relay: `stack status` shows whether the relay and daemon are up and which daemon identity is in effect.
   A relay that answers but delivers nothing usually means the clients were built for a different daemon
   identity; rebuild them.
5. Run `cleanup <run-id>` before starting again.

## 3. App regressions

1. `timeline.md` shows the exact step, the state each app displayed and the relay and wallet evidence around
   it. The `<actor>/steps/` screenshots and UI trees are named after those steps; the last one is
   `<scenario>-failed`. Start there and read backwards.
2. A form-derived observation carrying a different order ID than expected is a harness bug, not permission
   to proceed. The adapter must refuse foreign forms.
3. Contract failures are fixed on the app's automation branch and reviewed together with the adapter change.

## 4. Payment failures

The runner never retries once sats may have moved.

1. `wallet-payments.json` holds every reservation, in-flight payment with its hash, and settlement, plus
   run totals. `recovery.json` lists every expected payment with its direction.
2. An in-flight entry after the run means the wallet reported no terminal state within
   `run.settle_timeout_secs`. Look the hash up on the actor's node in **both directions**: an invoice the node
   issued and an outgoing payment it made. A hold invoice that settled later means the trade completed and
   the order must be checked through `cleanup`; a failed one needs no action. Unknown never means refunded.
3. Insufficient liquidity: `doctor` fails the wallet check when a node's balance is below
   `run.max_run_spend_sats`. Fix the stack; never raise the cap to make a run pass.

## 5. Cleanup and recovery

```sh
mortsom cleanup <run-id>
```

If the original journal has no business-action intents, or is already durably marked resolved, cleanup only
verifies stopped runtime markers and finishes storage removal. This offline path needs no stack, bundle or
UI session.

Otherwise cleanup opens the protected journal, checks that the current stack and captured bundle match,
restarts private runtime services on the retained profiles, verifies the original Alice and Bob public
identities, discovers uncertain submissions through their trade lists and cancels through ordinary UI
controls. Completed orders require both clients' terminal state and exact wallet settlement proof.
Cancelled orders require verified refund and cancellation of unused buyer invoices. Cleanup never retries a
payment or releases escrow.

Retained profiles live at `$HOME/.local/state/mortsom/recovery/<original-run-id>`. They are deleted only
after verified resolution and complete client shutdown. A new run is refused while a journal is unresolved,
whatever config or artifact directory is selected.

A killed coordinator leaves an active-runtime marker. Do not delete the marker, reset the stack or remove
the private directory to bypass the refusal: reconcile the owned client processes first. A stack mismatch
or missing protected files also refuses cleanup; keep the original stack available. `stack down` discards
the environment and is never proof of cancellation or refund. While a journal is pending and its stack
still exists, `stack up` and `stack down` are refused and name the `cleanup` to run first.

### Abandoning a journal whose regtest stack is gone

```sh
mortsom recovery abandon <run-id> --reason stack-destroyed --confirm-run <run-id>
```

This is the only way to close a journal that cleanup can no longer resume because the Mortsom-owned regtest
stack that held its orders was destroyed. `cleanup`, `stack up` and `stack down` print the exact command
when, and only when, every gate below is met. The command refuses otherwise:

- the journal is pending (a resolved or already abandoned journal has nothing to abandon);
- the journal recorded its environment: not the canary lane, mainnet spending never allowed, a
  Mortsom-owned stack directory, both actors on the stack's own LND nodes, both nodes reporting regtest;
- the original stack identity is recorded nowhere on the host (recorded directory, selected directory,
  Compose project). An unreadable identity or a Compose project without one fails closed;
- `--confirm-run` repeats the run id exactly.

Evidence is written first (`artifacts/<run-id>/recovery-abandonment.json` and `abandonment.json` in the
journal directory), then the journal turns terminal, then the retained profiles are removed. If removal
fails, rerun the same command; it retries removal without re-proving the gates. The run stays failed:
`cleanup` reports it as abandoned with exit code 1, `accounting` counts it as a leftover, `parity` never
accepts it and `publish` marks it failed. Abandonment never verifies an order or a payment and never counts
as a clean cleanup. Never delete or edit journals by hand.

## 6. Secrets

Node credentials and the daemon key live in owner-only files created by `stack up`. Configuration refers to
them through `env:` references; their contents never enter TOML, logs, build defines or artifacts. The
redactor removes configured values and key-shaped patterns from every artifact, and `publish` refuses a
bundle in which a configured secret survives. If a secret ever appears unredacted, treat it as exposed:
`stack down`, `stack up` and rebuild the clients for the new identity.

## 7. Incident response

1. Stop: do not start a new run until section 5 confirms nothing is open and section 4 confirms no payment
   is in flight.
2. Preserve: keep `artifacts/<run-id>/` and the private recovery directory. Use `publish` for a redacted
   bundle to share.
3. Classify with section 1 and file the issue in the right repository with the timeline.
4. Account: `accounting` shows settled sats, fees, leftovers and pass or fail per run. Leftovers must be
   explained.
5. Record the outcome in [docs/acceptance-ledger.md](acceptance-ledger.md).
