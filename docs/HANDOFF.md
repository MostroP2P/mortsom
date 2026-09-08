# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Decision of 2026-09-08

The operator chose to keep this repository and clean it rather than start over. The spec was rewritten as
v1.0. The plan, in order:

1. **Docs cut.** Done. Only README, spec, this file, the acceptance ledger and the runbook are live; the rest
   is under `docs/history/`.
2. **Close the live cells.** Linux done (2/2). Web blocked by the app; see below.
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

## Where things stand (2026-09-08, evening)

- **Linux 2/2 accepted**: `happy_sell` run `20260908T190151Z-d027fc8a5528`, `happy_buy` run
  `20260908T190424Z-85827b058763`. Both settled the hold on Alice and the payout on Bob exactly once.
- **Web 0/2, blocked by the app.** The Web build of app v2 has no trade persistence: `rust/src/db/indexeddb.rs`
  stubs `save_trade` and listing (app issue #233). Alice's Web client created order `3d2c95a2-…` but never
  listed it, so the harness could neither continue nor cancel through the UI. The pending order expires on the
  daemon after one hour with no funds involved.
- **A retained journal blocks new desktop runs.** Run `20260908T190651Z-8031670f7eb5` (Web) has an
  unresolved submission and its profile is retained under `$HOME/.local/state/mortsom/recovery/`. Until it is
  resolved, `run` refuses to start. `cleanup` cannot resolve it through the UI because the Web app cannot show
  the order. Options the operator must choose between: implement the app's trades store for Web and retry
  recovery; or add a relay-based resolution path for a maker submission whose order the app never listed
  (correlate the daemon's pending event in the submission window, then observe its expiry or cancel it once the
  app can); or, as an explicit operator decision, mark that journal resolved by hand after the order has
  expired on the daemon. None of these is done.
- Fixes landed today in the harness: LND empty-invoice-database lookup (`8de9f4e`); relay-confirmed
  cancellation with counterpart escalation (`aaa0615`, `869644a`); relay-list draw race in recovery
  (`30c3298`); public-book correlation uses `in-progress`, never `active` (`c39881e`).
- App v2 product issues found (to file against `MostroP2P/app`): optimistic `Canceled` on cooperative cancel
  request; no trade persistence on Web. Details in the acceptance ledger.

## Resume commands

```sh
git status --short && git log -8 --oneline
./target/debug/mortsom stack status
ls $HOME/.local/state/mortsom/recovery/          # 20260908T190651Z-8031670f7eb5 is the retained one

# Once the Web journal is resolved and the app persists trades on Web:
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

617 workspace tests passed, zero failed; formatting and strict clippy clean, at `c39881e`. Coverage has not
been measured at this checkpoint.
