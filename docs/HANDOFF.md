# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Decision of 2026-09-08

The operator chose to keep this repository and clean it rather than start over. The spec was rewritten as
v1.0. The plan, in order:

1. **Docs cut.** Done. Only README, spec, this file, the acceptance ledger and the runbook are live; the rest
   is under `docs/history/`.
2. **Close the first live cell.** Fix the LND lookup bug below, `cleanup` the retained run, retry Linux
   `happy_sell`, then `happy_buy`, then both on Web.
3. **Isolate the frozen Android path** (`crates/android-driver`, `crates/app-adapters/mobile-legacy`) behind
   a Cargo feature so the default workspace is desktop and Web only.
4. **Own the stack.** Replace the Ortsom delegation in `crates/coordinator/src/stack/` with Mortsom's own
   implementation of spec section 5. Needs its own short plan before starting.

## Spec versus code

The spec requires `mortsom stack up/status/down` to build the regtest stack without any external tool. The
code still delegates to a sibling Ortsom checkout through `--ortsom-dir` (default `../ortsom`), and
`config/desktop.example.toml` still resolves connection references from that checkout. Until step 4 lands,
the commands below work only with that checkout present and built. The README says so in its quick start.

## Workspace

| Project | Path | Branch |
|---|---|---|
| Mortsom | `/home/negrunch/dev/mortsom` | `feat/v2-desktop-web` |
| App v2 | `/home/negrunch/dev/mobilev2` | `feat/mortsom-linux-accessibility`, at `0ed7efb` |
| Ortsom (transitional dependency) | `/home/negrunch/dev/ortsom` | `feat/public-stack-status` |

Mortsom and app commits are local; nothing has been pushed from these branches. Run `git status --short`
and `git log -8 --oneline` in each before continuing.

## Environment

The persistent `ortsom-regtest` stack is up and intentionally left running: bitcoind, relay, mostrod 0.18.7
(protocol 2) and LND alice, bob and carol. Relay `ws://127.0.0.1:7000`; LND host ports 10001, 10002, 10003.
Use `stack status`, not `stack up`; `stack up` may regenerate the daemon identity, which requires rebuilding
both clients and is never evidence of cancellation or refund.

Both client bundles and the coordinator binary were rebuilt on 2026-09-08 against app `0ed7efb` and harness
`e846715`:

- Linux: `/home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro`
- Web: `/home/negrunch/dev/mobilev2/build/web`

Web on this host uses:

```sh
export MORTSOM_CHROMEDRIVER=/snap/chromium/current/usr/lib/chromium-browser/chromedriver
export MORTSOM_BROWSER=/snap/chromium/current/usr/lib/chromium-browser/chrome
```

Disk: about 34 GiB free on `/home`. Builds are large; check before coverage runs. Never delete artifacts or
private recovery state to gain space.

## Open item: run `20260908T171517Z-a54d8617c8eb`

The fourth Linux `happy_sell` attempt went further than any before: order `89b96c2f-…` created, taken by
Bob, buyer invoice provided, hold invoice paid by Alice. It then failed with category `wallet`:

```text
alice: LND lookup_invoice failed: status: Unknown, message: "there are no existing invoices"
```

Cause: `lookup_incoming` in `crates/environment/src/lnd.rs` treats only gRPC `NotFound` as "this node did
not issue that invoice" and falls through to the outgoing-payment lookup. LND 0.19 answers `Unknown` with
that message when the node's invoice database is empty, which is always the case for a seller who never
issued an invoice. The fix is to treat that answer as not-found as well, with a unit test for both codes.

State after the run: the order was cancelled through the UI during cleanup; no payment settled; Alice's hold
payment must be confirmed failed or cancelled on her node; protected profiles are **retained** and
`recovery.json` shows `resolution_verified=false`. Run `mortsom cleanup 20260908T171517Z-a54d8617c8eb` after
the fix and before any new run.

## Resume commands

```sh
git status --short && git log -8 --oneline
./target/debug/mortsom stack status

# After the LND fix:
cargo test -p mortsom-environment
cargo build -p mortsom-coordinator --bin mortsom
./target/debug/mortsom cleanup 20260908T171517Z-a54d8617c8eb

./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact /home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform linux \
  --app-artifact /home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_sell --platform web --app-artifact /home/negrunch/dev/mobilev2/build/web
./target/debug/mortsom run happy_buy  --platform web --app-artifact /home/negrunch/dev/mobilev2/build/web
```

Run one at a time. Check exit status, `summary.json` and `cleanup.json` before the next. On failure, fix the
observed cause, `cleanup` the run, record it in the acceptance ledger and retry with fresh actors.

If the app bundles must be rebuilt, build them sequentially; bridge generation writes shared source files:

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux
cargo run -p mortsom-coordinator --example prepare_client -- web
```

## Invariants to preserve

UI-only business actions. Rust harness; app-owned contract changes live in the app repository.
`mostro-core =0.14.5` pinned. Durable intent before any irreversible action; never resubmit an order or
retry an uncertain payment. Budget reserved before payment. Escrow settled is not completed. Cleanup restores
original identities, exact bundle and stack; missing or tampered storage refuses. SIGKILL fails closed.
Commit each completed task after fmt, strict clippy and meaningful tests.

## Last verification

613 workspace tests passed, zero failed, one existing ignored test; formatting and strict clippy clean, at
`e846715`. Coverage has not been measured at this checkpoint.
