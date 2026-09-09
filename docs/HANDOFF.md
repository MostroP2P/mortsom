# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Where things stand (2026-09-09, evening)

**The checkpoint is met: all four cells are accepted.** Linux `happy_sell` and `happy_buy`, Web `happy_sell`
and `happy_buy`, each a funded regtest trade driven end to end through the real app v2 UI, with the hold
settled once on Alice's node and the payout once on Bob's. Run ids are in
[the acceptance ledger](acceptance-ledger.md), which also records every failed attempt and its cause.

Everything from that work is merged. Mortsom `main` carries the desktop and Web harness (#17, #18), the
console step log (#20) and the banner (#21). The app's `main` carries the changes Web needed (#408, merge
`d49f3c1`): the IndexedDB stores, opening the store on the web at all, and the origin-wide coordination
around document writes. App issue #233 is closed; its leftovers are tracked in app issue #409.

**The stack is Mortsom's own.** `mortsom stack up` builds spec section 5 from `stack/` with Docker alone:
chain, three nodes with balanced channels to Carol, relay, and a daemon with an identity minted per stack.
Both Linux cells were re-run against it on 2026-09-09 (`20260909T175938Z-905f12907c4d` sell,
`20260909T180201Z-8743a4025779` buy), each settling the hold and the payout once and resolving its order.
The Web cells were not re-run on the new stack; the stack is the same for both platforms, and the Web bundle
must be rebuilt for the new daemon identity before a Web run.

No journal is retained, no harness process is left running, and the regtest stack stays up by design.

## What is left

1. **Isolate the frozen Android path.** `crates/android-driver` and `crates/app-adapters/mobile-legacy`
   still build by default. Put them behind a Cargo feature so the default workspace is desktop and Web only.
2. **Measure coverage.** The spec requires 80% of lines under `cargo llvm-cov`. Last measured at 83.88%
   several commits ago (`5c764ba`); it has not been re-run since.
3. **Further scenarios**, when the operator asks. The spec's deferred list (section 14) is the roadmap.

## Workspace

| Project | Path | Branch |
|---|---|---|
| Mortsom | `/home/negrunch/dev/mortsom` | `main` |
| App v2 | `/home/negrunch/dev/mobilev2` | `main` (PR #408 merged, `d49f3c1`) |

Ortsom's own stack (`ortsom-regtest`) was stopped, not destroyed, to free the shared host ports; `docker
compose -p ortsom-regtest start` brings it back, after `mortsom stack down`. The two cannot run together.

Run `git status --short` and `git log -8 --oneline` in each before continuing, and never discard unrelated
operator changes.

## Environment

The `mortsom-regtest` stack is up: bitcoind, relay, mostrod 0.18.7 (protocol 2) and LND alice, bob and
carol. Relay `ws://127.0.0.1:7000`; LND host ports 10001, 10002, 10003. Its state is under `stack/local/`.

Use `stack status`, not `stack up`. Every `stack up` mints a new daemon identity, which requires rebuilding
both clients, and is never evidence of a cancellation or a refund.

The Linux bundle was rebuilt for the current daemon identity from the app's `main`. **The Web bundle was
not**; rebuild it before a Web run. Build sequentially, because bridge generation writes shared source
files:

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux
cargo run -p mortsom-coordinator --example prepare_client -- web
```

They land at `/home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro` and
`/home/negrunch/dev/mobilev2/build/web`, and create no order and no payment.

Web on this host uses:

```sh
export MORTSOM_CHROMEDRIVER=/snap/chromium/current/usr/lib/chromium-browser/chromedriver
export MORTSOM_BROWSER=/snap/chromium/current/usr/lib/chromium-browser/chrome
```

Disk: about 23 GiB free on `/home`, 98% used. Builds are large; check before a coverage run. Never delete
artifacts or private recovery state to gain space.

## Running a live cell

```sh
git status --short && git log -8 --oneline
./target/debug/mortsom stack status
ls $HOME/.local/state/mortsom/recovery/     # a directory with `actors/` inside blocks new desktop runs

./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact /home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy  --platform web \
  --app-artifact /home/negrunch/dev/mobilev2/build/web
```

Run one at a time; check the exit status, `summary.json` and `cleanup.json` before the next. On failure, fix
the observed cause, `cleanup <run-id>`, record it in the acceptance ledger, and retry with fresh actors.

`cleanup` resolves a journal whose order the app never listed with `--attribute-order alice=<order-id>`, or
`alice=none` when the daemon created no order; both are checked against the daemon's own order events on the
relay for that run's window. A journal with no events at all is removed offline, without opening a client.

The console now prints each step, each observed screen state and each cleanup decision as it happens; the
full timeline, screenshots and accessibility trees stay in `artifacts/<run-id>/`. To watch the daemon at the
same time: `docker logs -f mortsom-regtest-mostro-1`, and `mortsom-regtest-lnd-carol-1` for its node.

## Known environment caveats

- Live Web runs are best launched detached from an agent session (`setsid nohup … &`) and followed through
  their log: a session's memory guard has killed background tasks, and once both browsers mid-trade. The
  funds were recovered by cooperative cancel. Nothing in the harness depends on this.
- A Web run whose browsers fail to start leaves an empty journal that blocks the next desktop run until
  `cleanup` removes it. If it happens twice in a row, check for a stray ChromeDriver from an earlier attempt.

## Invariants to preserve

UI-only business actions. Rust harness; app-owned contract changes live in the app repository.
`mostro-core =0.14.5` pinned. Durable intent before any irreversible action; never resubmit an order or
retry an uncertain payment. Budget reserved before payment. Escrow settled is not completed. A screen is
never taken as proof the daemon closed an order: the relay must show it. Cleanup restores original
identities, exact bundle and stack; missing or tampered storage refuses. SIGKILL fails closed. Commit each
completed task after fmt, strict clippy and meaningful tests.

## Last verification

649 workspace tests passed, zero failed, on the own-stack branch; formatting and strict clippy clean. App
v2 at `d49f3c1`: 343 Flutter tests passed, native and wasm32 clippy clean. Coverage has not been
re-measured since 83.88% at `5c764ba`.
