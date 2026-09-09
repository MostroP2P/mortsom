# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Decision of 2026-09-08

The operator chose to keep this repository and clean it rather than start over. The spec was rewritten as
v1.0. The plan, in order:

1. **Docs cut.** Done. Only README, spec, this file, the acceptance ledger and the runbook are live; the rest
   is under `docs/history/`.
2. **Close the live cells.** Done: Linux 2/2 and Web 2/2.
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

## Where things stand (2026-09-09, early morning)

- **All four checkpoint cells accepted.** Linux: `20260908T190151Z-d027fc8a5528` (sell),
  `20260908T190424Z-85827b058763` (buy). Web: `20260909T025026Z-034b13e4fb91` (sell),
  `20260909T033857Z-e7b88a390989` (buy). Every pass settled the hold once on Alice and the payout once on Bob,
  resolved its order and retained nothing.
- **App v2 changes required for Web** are on the app's `feat/mortsom-linux-accessibility` branch, pushed to
  `MostroP2P/app` as PR #408 (https://github.com/MostroP2P/app/pull/408), pending review: `fe7fd55` (IndexedDB trades and trade-key stores), `def4d40` (open
  the store on the web; `init_db` was skipped there, so nothing ever persisted), `e3ac458` (orders, relays,
  identity and outbox stores; the identity stub had made every order creation fail once the store was open).
  The Web bundle under test was built from `e3ac458`.
- **Harness changes on this branch** (`feat/web-happy-paths`): operator attribution for journals whose order
  the app never listed (`--attribute-order actor=order-id` or `actor=none`), relay-confirmed closure of
  unlisted orders, Web presses and focusing clicks that wait for the node to rest, verified picker choices,
  bounded settle for order-id readouts, re-entry of a dropped buyer invoice, cancel from invoice and payment
  forms, and providing the buyer invoice only after the daemon asks for it.
- **Environment caveat.** The Claude Code session's memory guard killed background tasks and, once, both
  browsers mid-trade (funds were recovered by cooperative cancel). Live Web runs were therefore launched
  detached (`setsid nohup … &`) and followed through their log. Nothing in the harness depends on this.
- No journal is retained; no harness process is left running; the regtest stack stays up by design.

Next steps, in order: land Mortsom PR #18 and app PR #408; then the remaining plan items (isolate the Android crates behind a
feature, replace the Ortsom delegation with the Mortsom-owned stack of spec section 5), coverage measurement
and the further scenarios the operator chooses.

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

630 workspace tests passed, zero failed; formatting and strict clippy clean, at `40377ea`. App v2: 343
Flutter tests passed, wasm32 and native clippy clean at `e3ac458`. Coverage has not been re-measured since
83.88% at `5c764ba`.
