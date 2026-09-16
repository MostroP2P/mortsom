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
controls. A submission whose order the trade list never showed is checked against the daemon's order
events on the relay for the run's window. The run already did this, right after the scenario and again
after the quiet window, whether or not the trade list could be read, so a journal reaches cleanup only when the relay showed a
candidate then; cleanup audits again after the retained clients ran and their trade lists were read: no
unknown order there resolves it as having produced nothing; otherwise cleanup prints
the candidates and the `--attribute-order` commands that settle it. Completed
orders require both clients' terminal state and exact wallet settlement proof.
Cancelled orders require verified refund and cancellation of unused buyer invoices. Cleanup never retries a
payment or releases escrow.

Retained profiles live at `$HOME/.local/state/mortsom/recovery/<original-run-id>`. They are deleted only
after verified resolution and complete client shutdown. A new run is refused while a journal is unresolved,
whatever config or artifact directory is selected.

A killed coordinator leaves an active-runtime marker. Do not delete the marker, reset the stack or remove
the private directory to bypass the refusal: reconcile the owned client processes first. A stack mismatch
or missing protected files also refuses cleanup; keep the original stack available. `stack down` discards
the environment and is never proof of cancellation or refund. While a journal is pending and its stack
still exists, or cannot be proved gone, `stack up` and `stack down` are refused and name the `cleanup` to
run first.

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
- `--confirm-run` repeats the run id exactly;
- the journal recorded its artifact directory and that directory still exists. Journals written before
  Mortsom recorded these facts are refused; the selected configuration is never proof of where a run's
  reports live. Such a journal stays pending; explicit continuation below does not abandon it (issue #43).

The command writes a prepared `recovery-abandonment.json` into the run's artifact directory, then
`abandonment.json` into the journal directory, then turns the journal terminal, then finalizes the public
record, then removes the retained profiles and bundle, printing every path it removed even when a later
removal fails. From the first write on, `cleanup`, `accounting`, `parity` and `publish` treat the run as
abandoned, whether the record is prepared, finalized or unreadable; a present record is never ignored. If
the command dies at any point, rerun it: a pending journal is proved again, a terminal one only finishes
the record and the removal. If instead `cleanup` verifies resolution of a journal whose abandonment died
before it turned terminal, cleanup discards only a valid prepared record for the same run and stack.
Cleanup retries that reconciliation even when the journal is already Resolved (issue #46), and removal
or directory-sync errors remain errors until a retry succeeds. Finalized, unreadable, unsafe or
mismatched evidence next to a resolved journal is preserved and refuses successful cleanup. An abandoned run stays failed:
`cleanup` reports it as abandoned with exit code 1, `accounting` counts it as a leftover, `parity` never
accepts it and `publish` marks it failed. Abandonment never verifies an order or a payment and never counts
as a clean cleanup. Never delete or edit journals by hand.

### Continuing while preserving ambiguous legacy recovery

For journals written before `RecoveryEnvironment`, there may be no trustworthy way to reconstruct the
old network, wallets or artifact location. Do not fill these fields from current configuration or mark
the old run resolved. The supported alternative is invocation-scoped continuation on a different stack:

```sh
mortsom run --tag smoke --platform linux \
  --app-ref https://github.com/MostroP2P/app/pull/456 \
  --preserve-legacy-recovery <legacy-run-id>
```

The option is an acknowledgement that recovery remains pending, not a declaration that the old stack
was regtest or destroyed. Repeat it for every exact pending legacy ID and for every new invocation.
There is no persisted bypass, automatic expiry, terminal transition, old-report rewrite or storage deletion.
The new run records the preserved IDs and fingerprints in `recovery-isolation.json`, with explicit
unverified wording, before any client starts. The old run remains unaccepted and its accounting is unchanged.

The command refuses unknown/duplicate/omitted IDs, modern pending journals, the same current stack
(including across fingerprint v1/v2), unreadable journals, missing/unsafe actor directories, and active
runtime markers. Never remove a marker to make it pass. It verifies current Compose ownership, descriptor
integrity, both configured stack LND endpoints and live regtest networks. Current mainnet permission or
canary always refuses. Wallet networks are rechecked before clients start even for non-payment scenarios.
If the new run fails and leaves recovery, use ordinary `cleanup <new-run-id>` before another scenario.
Budgets, caps, fresh identities and the host mutation lock are unchanged.

Legacy stack descriptors can be read in place with their original fingerprint verified and daemon source
explicitly shown as unrecorded. Upgrading the descriptor format alone never proves a different stack.
Stack lifecycle warnings name continuation as conditional guidance; they do not establish the old network
or destruction. Continue to resolve current-stack recovery before `stack up`, `--replace` or `stack down`.
Manual acceptance of the legacy incident remains required; do not automatically close issue #43.

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
