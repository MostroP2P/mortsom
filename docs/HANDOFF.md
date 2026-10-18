# Handoff: current state and how to resume

Read [spec.md](../spec.md) first; it is normative. This file says where the work is and what comes next.
Repository content is English; the operator conversation is Spanish.

## Where things stand (2026-10-18)

**The anti-abuse bond family (spec 8.8) is implemented and ran live on Linux: 14 of 15 pass.** The
fifteenth, `taker_bond_expires_unpaid`, fails on the app: a taker's lapsed bond stays in My Trades
(MostroP2P/app#849, fixed on the app's `fix/taker-bond-lapse`). It runs on its own stack profile,
`stack up --bond` (item 9 below), against mostrod v0.20.0. Run ids are in
[the acceptance ledger](acceptance-ledger.md). Web was not run (no Chromium on this host).

The stack on this host is the bonded one (daemon `ef1ca75a…`, mostrod `main` at `f1f7a62`); every other
family is skipped on it until `stack up --replace`. No recovery journal is pending. A run started two hours
after the newest block found mostrod drained; until #87 lands, mine a block first (runbook 2.5).

## Where things stood (2026-10-14)

**Linux is re-verified against app `main` (`ae3fe214`) and mostrod v0.19.3: 26 passed, 0 failed, 8 skipped.**
That covers the twenty cells of the five specified families, plus, for the first time live,
`unauthorized_actions_at_taken` and the five `restart_*` relaunches. The seven interruption scenarios and
`nwc_cancel_payment` are skipped with their reason. **Web was not re-run**: this host has no Chromium (see
*What is left*). Run ids are in [the acceptance ledger](acceptance-ledger.md).

Keeping up with the app took ten open PRs. Merge them **in this order**; the stacked ones retarget to `main`
as their base merges:

| PR | Base | What |
|---|---|---|
| #67 | `main` | Identifier contract test reads `MORTSOM_APP_CHECKOUT`; CI checks out app `main` |
| #68 | `main` | Pick the stack's daemon on the app's first-run node choice (and read a merged Linux node by its own line) |
| #69 | #68 | Wait for the book a new order's close returns to |
| #70 | #69 | `go_home` on a screen still drawing; cleanup moves on when the maker already asked to cancel |
| #72 | #70 | Read a trade's actions on the form it opens on |
| #73 | #72 | Skip a scenario needing an interruption the run cannot cause (`Need`) |
| #71 | `main` | A dispute the seller released under closes as `released` (mostro-core 0.17) |
| #74 | `main` | mostrod v0.19.3 and `mostro-core =0.17.3` (`user-info`, which app `main` sends on every launch) |
| #75 | #74 | Tell the launch's read-only queries from a repeated trade action (daemon key, owned stack only) |
| #76 | `main` | The Web doctor starts the browser, not only finds it |

The stack on this host was rebuilt on v0.19.3 on 2026-10-14 (daemon `6f11dd42…`); no recovery journal is
pending.

## Where things stood (2026-09-25)

**Every implemented family is accepted live on both platforms: Linux 20/20, Web 20/20.** Happy path,
cancellations, expirations, disputes and payment failures; the three payment-failure scenarios are accepted
on Web for the first time. Run ids, and every failed attempt with its cause, are in
[the acceptance ledger](acceptance-ledger.md).

On 2026-09-24/25 the cancellation, payment-failure, dispute and expiration families were re-verified
against app `main` (`f15e6bc`, then `42019bd` for `payout_invoice_below_expiry_floor`). Keeping up with the
app and making the harness hold on a slower host took these changes, all merged:

- **Linux on Ubuntu/Pop!_OS 22.04 with GNOME** (#52): a private accessibility bus per client.
- **The app's redesigns**: the payment-method screen and price segments of create-order (#53); a cancelled
  never-active trade leaving My Trades, proven by the relay and a loaded list (#54); the rating on the trade
  detail itself (#56); identifiers the app no longer has dropped from the adapter (#58).
- **Web robustness**: a press awaits a click already sent (#55); a label read re-finds a node the app
  replaced (#57); a text entry is read back even when the browser answers past the deadline (#61); the
  client is built without the resources CDN, so CanvasKit loads from the run's loopback origin (#63).
- **Recovery**: empty submissions ruled out by the relay (#49, #50); a dropped never-active order's
  payments verified by the relay (#60).
- **`doctor`** checks the C toolchain Flutter's Linux build takes from `clang++` (#59).
- **The app's own invoice verdict** (#64, with MostroP2P/app#588): `invoice.check` names the rule an invoice
  breaks before the app sends it; the adapter waits for that word, and `payout_invoice_below_expiry_floor`
  accepts the app's refusal or, while the node's capabilities have not reached the app, the daemon's.
- **Reports** show how long each scenario took (#51).

The regtest stack was rebuilt on 2026-09-25 (daemon `9cc66bab…`) to close a journal cleanup could not
resolve (see the caveats below). It is up, and no recovery journal is pending on this host.

## What is left

1. **Cleanup has no way out of a failed payout awaiting a replacement.** When a scenario stops after the
   escrow settled and before the buyer hands in a new invoice, the daemon holds the payout and only an
   invoice completes the order. Cleanup never hands one in, and `stack down` refuses while that journal's
   stack exists. Decide between documenting the manual path (below) in the runbook and letting cleanup
   complete the order in regtest, which changes spec section 11.
2. **Web is not re-verified against app `main`.** This host's `chromium-browser` is Ubuntu's wrapper for
   the Chromium snap, which is not installed, and there is no `chromedriver`. Install the snap (`sudo snap
   install chromium`; Mortsom uses the unconfined binaries inside it) or point `MORTSOM_BROWSER` and
   `MORTSOM_CHROMEDRIVER` at a matching pair, then run the catalogue on Web against the merged PRs above.
3. **A batch's report is only printed.** A multi-scenario `run` prints its family report to the console;
   each run keeps its own artifacts, but nothing records the batch (what was asked, what never ran, the
   totals). Write it under `artifacts/` and name its path.
4. **Fixed 5 s bounds in `web.rs`** for presses and text entry, instead of the configured UI timeout. They
   no longer cut an action in flight.
5. **Files over the 800-line limit**: `crates/coordinator/src/run/tests/mod.rs` and
   `crates/scenario-kit/src/actions.rs`; `scenarios/src/flow_test_adapter.rs` is close.
6. **Re-run CI on `main` after 2026-10-01.** The organization's GitHub Actions minutes ran out; every PR
   since #54 was verified locally only (see *Last verification*).
7. **Open issues**: #46 (retry prepared abandonment cleanup after journal resolution) and #48 (split the
   public core from private adversarial scenarios).
8. **Further families**, when the operator asks: adversarial messages (spec section 8.1; section 14 is the
   roadmap), and what the bond family leaves out (spec 8.8): the lock race between two takers needs a third
   actor; a forfeited claim and a slash the node keeps whole need other stack policies; a range maker's bond
   is sized from the market price. Chat, dispute chat, NWC, account backup and import, and notifications
   have no scenarios yet: the app's notification cards carry no automation identifiers, so neither the
   "Order published" card nor the bond notices can be read black-box.
9. **The bond family runs on its own stack.** `stack up --bond --replace` for it, `stack up --replace` for
   every other family; each mints a new daemon identity and so a new client build (spec 8.1). The ported
   September work (`feat/bond-family`, never merged, 29 commits) is folded into `feat/bond-scenarios`; that
   old branch is kept locally for reference only.

## Running a live cell

```sh
git status --short && git log -8 --oneline
./target/debug/mortsom stack status
./target/debug/mortsom doctor --platform linux     # or web; performs no payment, creates no order
ls $HOME/.local/state/mortsom/recovery/            # a journal not closed blocks new desktop runs

./target/debug/mortsom run happy_sell --platform linux
./target/debug/mortsom run --tag payment-failure --platform web
./target/debug/mortsom run                        # every scenario, one run each
./target/debug/mortsom run --tag smoke --app-ref https://github.com/MostroP2P/app/pull/<n>
```

Without `--app-source` or `--app-artifact`, `run` fetches the app's `main` (or `--app-ref`), builds it for the
stack's daemon identity and captures the bundle; no prebuilt client is needed. Use `stack status`, not `stack
up`: every `stack up` mints a new daemon identity. With no option `stack up` builds Mostro's `main` at its
current head (several minutes the first time); `--mostro-ref <branch-or-pr-url>` builds another head, and
`--mostro-image mostrop2p/mostro:v0.19.3` runs the release the 2026-10-14 cells ran against. `doctor` checks
the host's build prerequisites, including `lld`, `protoc` 3.15 or later and the C toolchain beside `clang++`
(README, *Requirements*).

Check the exit status, `summary.json` and `cleanup.json` before the next run. On failure, fix the observed
cause, `cleanup <run-id>`, record it in the ledger, and retry with fresh actors. `cleanup` resolves a journal
whose order the app never listed with `--attribute-order alice=<order-id>`, or `alice=none` when the daemon
created no order; both are checked against the daemon's own events on the relay. Set
`MORTSOM_WEBDRIVER_VERBOSE=1` to keep ChromeDriver's full log as `driver.log` on Web.

To watch the daemon at the same time: `docker logs -f mortsom-regtest-mostro-1`, and
`mortsom-regtest-lnd-carol-1` for its node.

## Known environment caveats

- **A failed payout awaiting a replacement blocks recovery** (item 1 above). The 2026-09-25 journal
  `20260925T005654Z-698c1a59c920` was closed this way, as a regtest-only procedure: stop the Compose project
  exactly as `stack down` does (`COMPOSE_FILE=stack/compose.yml COMPOSE_PROJECT_NAME=mortsom-regtest docker
  compose --profile daemon down -v --remove-orphans`), remove `stack/local/`, then `mortsom recovery abandon
  <run-id> --reason stack-destroyed --confirm-run <run-id>` and `mortsom stack up`. The run stays failed and
  unverified. Never delete or edit recovery journals by hand.
- Live Web runs are best launched detached from an agent session and followed through their log: a
  session's memory guard has killed background tasks, and once both browsers mid-trade.
- The host going to sleep kills both Chromium sessions mid-trade; `cleanup` recovers the funds. Keep the
  machine awake during Web families.
- A Web run whose browsers fail to start leaves an empty journal that blocks the next desktop run until
  `cleanup` removes it. If it happens twice in a row, check for a stray ChromeDriver from an earlier attempt.
- **A stack whose newest block is about two hours old stops trading** (runbook 2.5): LND reports itself
  unsynced, and mostrod rejects new orders and takes and pays no bond claim. Mine one block and wait five
  minutes for the daemon to leave maintenance mode. Seen on 2026-10-18 in the second bond family run, which
  started 1 h 46 min after `stack up`; #87 is to make the run keep the tip fresh itself.

## Invariants to preserve

UI-only business actions. Rust harness; app-owned contract changes live in the app repository.
`mostro-core =0.17.3` pinned. Durable intent before any irreversible action; never resubmit an order or
retry an uncertain payment. Budget reserved before payment. Escrow settled is not completed. A screen is
never taken as proof the daemon closed an order: the relay must show it. Cleanup restores original
identities, exact bundle and stack; missing or tampered storage refuses. SIGKILL fails closed. Commit each
completed task after fmt, strict clippy and meaningful tests.

## Last verification

On 2026-10-18, on `feat/bond-scenarios` (Rust 1.99.0): formatting and strict workspace clippy clean, 1037
workspace tests passed, each commit compiling on its own. Live: the bond family on Linux, as in the ledger.
GitHub Actions has not run on it.

### Before (2026-10-14)

On 2026-10-14, on the local integration of #67 to #76 (Rust 1.99.0): formatting and strict workspace
clippy clean, 864 workspace tests passed (each PR branch also passes on its own). Live: the Linux catalogue above, `20261014T045514Z-5722ecfe2ca9`
to `20261014T062553Z-fb7e82c618c5`. GitHub Actions has not run on these PRs.

### Before (2026-09-25)

On `main` at `3f3a96b` (#64), with the Rust toolchain CI uses (1.98.1): formatting and strict
workspace clippy clean, 839 workspace tests passed, 84.96% line coverage under `cargo llvm-cov`
excluding scenario bodies (floor 80%). GitHub Actions has not run since #54 (no minutes until
2026-10-01). Last live runs: `payout_invoice_below_expiry_floor` on Linux
(`20260925T191709Z-1808d0cf2260`) and Web (`20260925T185803Z-e5379658a4a9`), and `cancel_before_taken`
on Web with the CDN-free build (`20260925T151914Z-5214b0d3eac9`).

## History

- **Managed app preparation (2026-09-15).** The default source is managed `MostroP2P/app` main; git objects
  are cached in ignored `.mortsom/app/repository.git` and each build checkout is removed after capture.
  `--app-source <path>` keeps an explicit checkout, `--app-artifact <path>` skips the build (spec section 7).
- **Recovery abandonment (2026-09-16, #45).** `mortsom recovery abandon <run-id> --reason stack-destroyed
  --confirm-run <run-id>` is the only terminal path for a journal whose Mortsom-owned regtest stack is
  provably destroyed; `stack up`/`stack down` refuse while a pending journal's stack exists (spec section
  11, runbook section 5). Code: `crates/coordinator/src/recovery_abandon/`, `cli/abandon.rs`. First used
  live on 2026-09-25.
- **Legacy recovery continuation (2026-09-16, #47).** A journal written before `RecoveryEnvironment` was
  recorded cannot be abandoned; `run --preserve-legacy-recovery <run-id>` continues on a different, verified
  regtest stack and leaves it pending. The legacy journal `20260915T180600Z-75d6ac262087` was pending on the
  host where it was recorded; it is not on this one.
- **Families and own stack (2026-09-08 to 2026-09-10).** `mortsom stack up` builds spec section 5 with
  Docker alone. The dispute family registers a solver per run from the daemon key (`crates/solver`); the
  expiration family relies on the declared `order_expiration_secs` (600) passed to the build. The live runs
  of that period, and the app changes they needed, are in the ledger.
