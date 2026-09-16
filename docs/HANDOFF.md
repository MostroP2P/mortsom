# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Legacy recovery continuation (2026-09-16)

The baseline is main `5dc7d64`, including merged PR #45 and PR #44. The retained run
`20260915T180600Z-75d6ac262087` has no `RecoveryEnvironment`. The reproduction confirmed a ready
replacement stack, a smoke invocation against app PR #456 that started no scenarios, changed-stack
cleanup refusal, and correct abandonment refusal. No historical environment facts can be backfilled.

`run --preserve-legacy-recovery <run-id>` now offers explicit continuation on a different, verified
Mortsom regtest stack. Repeat for all pending legacy journals on every invocation. The original journal
and profiles remain pending and unchanged. Current ownership, stack binding, non-canary lane, disabled
mainnet permission and both live regtest nodes are required; actor shutdown markers still fail closed.
New runs retain ordinary cleanup and budget requirements and write `recovery-isolation.json` plus a
warning. A newly pending journal blocks further work. See spec section 11 and runbook section 5.

The existing replacement stack used a schema-1 descriptor. Read-only compatibility validates its
fingerprint without inventing daemon provenance; v1/v2 comparisons prevent a schema upgrade from
looking like stack destruction. Issue #46 is also covered: already-Resolved cleanup retries valid
Prepared-marker removal and refuses contradictory or unremovable evidence.

Validation: formatting and strict workspace/all-targets clippy passed; 770 workspace tests passed with
two existing ignored tests and only `selectors_match_the_v2_app_contract` filtered (the unfiltered run
fails on the sibling checkout's missing `node.use_default`, as noted below). The same instrumented suite
passed with 84.00% line coverage, excluding scenario bodies as in CI. Independent CodeRabbit review
reported no remaining findings after removing a descriptor double-read.

The first post-change Linux smoke built app PR #456 at `2724772a530f4c3b2826d6ec881309480c613bba`,
then exposed a report-startup rejection of the legacy descriptor's unrecorded daemon source. A runner
regression reproduced it before the fix; reports now preserve that explicit unknown while modern
descriptor validation remains strict. Both Linux and Web runner tests cover that combination.

The final live Linux smoke on the same app revision created `20260916T155537Z-af7ee1867161`, recorded
isolation evidence and launched both clients. It did **not** pass: both clients timed out waiting for
`keys.public_key` during onboarding, before any scenario body or order/payment event. Its new pending
journal correctly blocked the next family member. Ordinary cleanup succeeded offline, resolved that
new empty journal and removed its inactive storage; failed reports remain in artifacts. The original
legacy manifest is unchanged (SHA-256 `226aa0c69ab3fe719d343aa298eb02f23b76d601efdd6a751deeff40eeac1be7`),
still pending and unverified. PR #47 records the evidence; CI passed format/lint, tests and coverage on
`0426636`. App onboarding and live Linux/Web business-flow acceptance remain outstanding.

Manual acceptance for issue #43 is still required. Keep PR language non-closing until it is recorded;
automated tests and agent-run smoke are not a substitute for that acceptance.

## Recovery abandonment (2026-09-16)

PR #45 (`feat/recovery-abandon`) adds `mortsom recovery abandon <run-id> --reason stack-destroyed
--confirm-run <run-id>`, the only terminal path for a journal whose Mortsom-owned regtest stack was
destroyed (spec section 11, runbook section 5). The journal now records `RecoveryEnvironment` at creation
(lane, mainnet permission, stack directory, artifacts directory, wallet backend, node networks) and
`stack up`/`stack down` are refused while a pending journal's stack exists or cannot be proved gone.
Abandonment is a failed run everywhere: `cleanup`, `accounting`, `parity` and `publish` treat any
`recovery-abandonment.json`, prepared, finalized or unreadable, as abandonment. Code lives in
`crates/coordinator/src/recovery_abandon/` and `cli/abandon.rs`.

**Scope: journals created after this change only.** The retained legacy journal that motivated issue #43
predates the recorded environment and is still refused; its stored evidence does not record the original
stack directory, mainnet permission, wallet-to-stack binding, node networks or artifact directory, and no
trustworthy source for them exists, so no backfill was attempted. The continuation above preserves
that incident unresolved; no automatic closure of issue #43 is intended.
A live acceptance of the abandon path on a newly recorded journal is still to be run. The adapter test `selectors_match_the_v2_app_contract` reads the sibling `../mobilev2`
checkout and fails there for a missing `node.use_default` id; it is unrelated to this branch.

## Managed app preparation (2026-09-15)

The default source is now managed `MostroP2P/app` main. `run --app-ref <branch-or-PR-URL>` resolves and
builds its exact head once for the selected scenarios; `--app-source <path>` preserves explicit developer
checkouts and `--app-artifact <path>` bypasses source preparation. Git objects are cached in ignored
`.mortsom/app/repository.git`; disposable clean build checkouts are removed after capture. README and
spec section 7 describe the current workflow. The environment and local artifact paths below are
historical observations, not assumptions about the machine today: recheck `stack status` before live work.

## Where things stand (2026-09-10)

**Five families are implemented and accepted live on Linux (20/20), the first four also on Web (17/20):
happy path, cancellations, expirations, disputes and payment failures.** The dispute family (Mortsom `feat/dispute-family`, app
`feat/mortsom-dispute-back-id`) adds `crates/solver`: each run that needs one mints a solver identity,
registers it with `admin-add-solver` sent from the daemon key read from `stack/local/config/settings.toml`
(the message needs an id or the daemon drops it silently), and rules with `admin-take-dispute` (dispute
id) and `admin-settle`/`admin-cancel` (order id) on the dispute wrapper — the shapes the reference
client uses. Scenarios see it as `Context::solver`; the proof of each step is the daemon's kind-38386
event after a baseline. `cleanup` on an owned stack registers a solver too, so a disputed leftover is
cancelled by it. The operator tool `cargo run -p mortsom-solver --example solve -- <relay> <daemon-pubkey>
stack/local/config/settings.toml <dispute-id> [cancel|settle]` rules on one dispute by hand. The
expiration family needed a stack rebuilt for the declared
`order_expiration_secs` of 600 (daemon `6e18952f…`) and clients built from app branch
`feat/mortsom-order-expiry-define` (`MORTSOM_ORDER_EXPIRY_SECS`, which the build asks the daemon for on
every order). An open Web flake is recorded in the ledger: Alice's Chromium died twice as her client applied
the expiry of her own pending order; run that scenario with `MORTSOM_WEBDRIVER_VERBOSE=1` to catch it. Spec section 8 lists
the seven scenario families with their tags; the first two are in the catalogue: `range_order`,
`market_price_order`, `full_trade_with_rating` (with `happy_sell`, `happy_buy`) and `cancel_before_taken`
(was `cancel_pending`), `cancel_after_take_unpaid`, `taker_cancel_at_taken`, `cooperative_cancel`,
`unilateral_cancel_rejected`. `run --tag <family>` on Linux and Web runs the family as a sequence of
single-scenario runs and prints a family report; the run keeps each client's own log as
`artifacts/<run>/<actor>/app.log` (`driver.log` on Web).

The live runs found and fixed three app v2 gaps, all on the app branch `feat/mortsom-range-and-rating-ids`
(PR MostroP2P/app#410, on top of `main` at `d49f3c1`): the range and star identifiers (`20e7e90`); a maker's trade
stuck at `waiting-payment` after the taker walked away, because `new-order` was only applied to a create
being waited for (`8e12332`); and range orders never leaving a remainder, because the release carried no
`NextTrade` and no client would have adopted the child (`83d9b8a`, `2917ecc`, `e8d7c06`). The ledger has
every attempt with its cause. The stack was rebuilt for this work (`mortsom stack up`, daemon
`f17567fc…`), and both clients were rebuilt from the app branch.

**The payment-failure family is implemented** (Mortsom `feat/payment-failure-family`, app
`feat/mortsom-payout-failure`): `payout_fails_then_replaced`, `payout_invoice_below_expiry_floor` and
`payout_slots_capped_per_destination` (spec section 8.6). Bob's invoices are shaped on the node Mortsom
holds for him -- withdrawn before the release, issued with a chosen lifetime, or issued as hold invoices
(`LndWallet::make_hold_invoice`, `settle_hold_invoice`, `invoice_state`) -- and the declared limits reach
scenarios as `Context::limits` from the stack descriptor. A withdrawn or refused invoice is journaled as
`PaymentSuperseded` before the node cancels it, so a replacement can be prepared for the same order and
recovery verifies the old one never settled. The app change was needed because the daemon's
replacement request (`add-invoice` on a `settled-hold-invoice` order) was persisted as that status and
never shown; the app now treats it as a request for an invoice, maps `invoice-updated` back to
`payout-pending`, and keeps a refused invoice's reason on screen as `invoice.error` (see the app's
`docs/automation-contract.md`, "Buyer payout failure"). Build the clients from that app branch. All
three scenarios are accepted on Linux (2026-09-10, ledger); Web was not run. The capped-payouts scenario
exposed an app quirk on the second create form of a session (submit stays disabled with every field
filled); the adapter now leaves and refills the form once, and the ledger records it as an open app issue.

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

1. **Measure coverage.** The spec requires 80% of lines under `cargo llvm-cov`. Last measured at 83.88%
   several commits ago (`5c764ba`); it has not been re-run since.
2. **Land the dispute branches.** App `feat/mortsom-dispute-back-id` (the dispute screen's `appbar.back`)
   and Mortsom `feat/dispute-family`. Until the app PR is merged, build the clients from that branch.
3. **Land the payment-failure branches** (app `feat/mortsom-payout-failure`, Mortsom
   `feat/payment-failure-family`) and record the live cells in the ledger.
4. **Further families**, when the operator asks: anti-abuse bonds and adversarial messages (spec
   section 8.1; section 14 is the roadmap).
   The buy-side range remainder is driven by `range_buy_order` (app branch `feat/mortsom-range-buy-remainder`
   on top of #410, Mortsom branch `feat/range-buy-order`).

## Workspace

| Project | Path | Branch |
|---|---|---|
| Mortsom | `/home/negrunch/dev/mortsom` | `main` |
| App v2 | `/home/negrunch/dev/mobilev2` | `feat/mortsom-range-and-rating-ids` at `e8d7c06` (PR #410 against `main`) |

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

With the current default configuration these examples fetch managed main, build and verify a temporary
captured bundle, then remove the checkout/build and capture. They leave no reusable app artifact and
create no order/payment. They honor `MORTSOM_STACK_DIR` and accept an optional branch/PR URL after the
platform. The historical `/home/negrunch/dev/mobilev2/build/...` paths apply only to builds explicitly
configured with `mobile.source_dir = "/home/negrunch/dev/mobilev2"`; verify those artifacts still exist
and match the stack before using the historical commands below.

Web on this host uses the Chromium snap. Mortsom detects the confined `/snap/bin/chromium` wrapper and
runs the unconfined `chrome` and `chromedriver` inside the snap by itself; `MORTSOM_BROWSER` and
`MORTSOM_CHROMEDRIVER` are no longer needed here, and still win when set.

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
- The host going to sleep kills both Chromium sessions mid-trade (2026-09-10 01:29, `systemd-suspend` in
  the system log); `cleanup` recovers the funds. Keep the machine awake during Web families, or run the
  family one scenario at a time so a dead run stops less.

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
