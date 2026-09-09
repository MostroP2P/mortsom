# Acceptance ledger

One row per scenario and platform. A cell is **accepted** only after a live run through the real app v2 UI
proves the assertions in [spec.md](../spec.md) section 2 and finishes verified cleanup. Builds, unit tests,
onboarding, identity probes, cancellation diagnostics and skipped cases never count.

**Accepted: Linux 2/2, Web 2/2.** All four checkpoint cells are accepted.

| Scenario | Linux | Web |
|---|---|---|
| `happy_sell` | **Accepted** 2026-09-08, run `20260908T190151Z-d027fc8a5528` | **Accepted** 2026-09-09, run `20260909T025026Z-034b13e4fb91` |
| `happy_buy` | **Accepted** 2026-09-08, run `20260908T190424Z-85827b058763` | **Accepted** 2026-09-09, run `20260909T033857Z-e7b88a390989` |

## Live attempts

Newest first. Run artifacts are under gitignored `artifacts/<run-id>/`.

| Run | Platform | Scenario | Reached | Failure |
|---|---|---|---|---|
| `20260909T180201Z-8743a4025779` | Linux | happy_buy | **Passed** on the Mortsom-owned stack (`mortsom stack up`, no Ortsom checkout) | None. Hold and payout settled once each; order resolved |
| `20260909T175938Z-905f12907c4d` | Linux | happy_sell | **Passed** on the Mortsom-owned stack (`mortsom stack up`, no Ortsom checkout) | None. Hold and payout settled once each; order resolved |
| `20260909T122846Z-31d9701a9752` | Web | happy_sell | **Passed** on app `f1f995f` (book event checked for order id, lock acquisition races the request promise) | None. Hold and payout settled once each |
| `20260909T114337Z-b620dfafd3fc` | Web | happy_sell | **Passed** on app `59a047e` (origin-wide Web Locks around document writes, guarded payout write, author check on fetched events) | None. Hold and payout settled once each |
| `20260909T110604Z-a5d6eaee777a` | Web | happy_buy | **Passed** on app `0e65bd0` (seller completes from the public success event) | None. Hold and payout settled once each. Part of the 2026-09-09 four-cell timing batch |
| `20260909T103926Z-8587e76354b9` | Web | happy_buy | Daemon paid Bob; Alice's Web client stayed on `payout-pending`, also after the first recovery restart | Root cause found: the daemon sends `PurchaseCompleted` to the buyer only; the seller learns of the payout from the public `success` event, and the app's stale sweep ignored `SettledHoldInvoice`, so a missed event was never repaired. App `0e65bd0` confirms the payout against the book for a minute after the escrow settles and lets the sweep complete such trades. Harness `7966a5b` resolves such a journal on the daemon's `success` plus one settled record per hash. Resolved by cleanup, both settlements verified |
| `20260909T103509Z-4b2f0aabf959` | Web | happy_sell | **Passed** on app `93719ec` | None. Part of the timing batch |
| `20260909T102900Z-eda642371033` | Web | happy_sell | Both browsers closed at the release step with the hold in flight | Environment: both Chromium processes died simultaneously; cooperative cancel through recovery released the 1005 sats. Cleanup healthy |
| `20260909T102645Z-b25c3889174b` | Linux | happy_buy | **Passed** on app `0e65bd0` Linux bundle | None. Part of the timing batch |
| `20260909T102431Z-d0d733b2c3a9` | Linux | happy_sell | **Passed** on app `0e65bd0` Linux bundle | None. Part of the timing batch |
| `20260909T101325Z-f250eea4b457` | Web | happy_sell | **Passed** on app `93719ec` (review fixes on app PR #408: serialised IndexedDB patches, spec aligned) | None. Hold and payout settled once each. Harness `4bb36bd` |
| `20260909T100500Z-dfd813ba075c` | Web | happy_sell | Alice's pay screen never rendered after Bob's invoice | App build `b8e3883` held one IndexedDB transaction across awaits; the write hit an inactive transaction and the hold invoice was never stored. Replaced by an async lock in `93719ec`. Cleanup healthy, no payment |
| `20260909T095315Z-d4be48bb4aaf` | Web | happy_sell | Daemon reached `success` and paid Bob; Alice's Web client stayed on `payout-pending` for 90 s | The live `PurchaseCompleted` message was not applied by Alice's client until it restarted; `cleanup` reopened both clients, saw completed on each and verified both settlements once. App product flake to watch (build `e3ac458`) |
| `20260909T065642Z-a9d467b16107` | Web | happy_buy | **Passed** again after the review fixes on PR #18 (relay-audited attributions, invoice request observed as the trade reopening on its form) | None. Hold settled once on Alice, payout once on Bob. Harness `4bb36bd` |
| `20260909T033857Z-e7b88a390989` | Web | happy_buy | **Passed.** Order `e03c537d-30ac-4f47-82cd-35ed988b4e8c`: Bob created the buy, Alice took and paid hold `353bb042…`, Bob provided the invoice once the daemon asked, fiat sent, released; both clients completed | None. Hold settled once on Alice (1005 sats), payout once on Bob (995 sats). Resolved, nothing retained. Harness `40377ea`, app `e3ac458` |
| `20260909T025026Z-034b13e4fb91` | Web | happy_sell | **Passed.** Order `0620785e-a819-487b-8c04-dfbd4822c716`: created, taken, invoice, hold `e185d552…` paid, fiat sent, released; relay `success`; both clients completed | None. Hold settled once on Alice (1005 sats), payout once on Bob (995 sats). Resolved, nothing retained. Harness `123ff35`, app `e3ac458` |
| `20260909T02*` and `20260909T03*` (nine runs) | Web | both | Diagnostic attempts between the two passes | Each was recovered with verified cleanup and no settled payment: two browser deaths under the environment's memory guard (one with a hold in flight, released by cooperative cancel), a lost submit press, a currency pick the picker ignored, a focusing click landing on the currency row, a dropped buyer invoice, an order-id readout read mid-transition, and two submissions that produced no order (closed with `--attribute-order actor=none`). Fixes: `1e4ae9a`, `123ff35`, `c0faa5a`, `2f2361c`, `27b8bf2`, `40377ea` |
| `20260909T005557Z-bf36f2d729bf` | Web | happy_sell | Alice submitted the sell order; the daemon saved `2ff23833-e73f-4ef3-8387-4dc5e0ebf37b` | Same symptom as the run below with the IndexedDB trades store already built in (app `fe7fd55`): the Dart bootstrap never called `init_db` on the web, so `db()` stayed unset and nothing persisted. Fixed in app `def4d40`. The journal was resolved with `cleanup --attribute-order alice=…` once the daemon expired the order; no payment |
| `20260908T190651Z-8031670f7eb5` | Web | happy_sell | Alice submitted the sell order; the daemon saved `3d2c95a2-1209-4e9a-b7b1-10150154213d` as pending | Alice's Web app never listed the trade: "Mis operaciones" stayed empty. App v2's IndexedDB backend (`rust/src/db/indexeddb.rs`) implements only chat messages and settings; `save_trade`, `get_trade` and trade listing are stubs pending the app's issue #233. No payment. The pending order expires on the daemon after its one-hour limit. The journal has an unresolved submission and its Web profile is **retained**; see HANDOFF |
| `20260908T190424Z-85827b058763` | Linux | happy_buy | **Passed** on the first attempt. Order `c31c3647-2098-4b5a-a3e7-8a3b32831cdc`: Bob created the buy, Alice took it and paid hold `ee711130…`, Bob provided invoice `fbca5acb…`, fiat sent, released; both clients completed | None. Hold settled once on Alice (1005 sats), payout settled once on Bob (995 sats), zero fees. Resolved, nothing retained. Harness `c39881e`, app `0ed7efb` |
| `20260908T190151Z-d027fc8a5528` | Linux | happy_sell | **Passed.** Order `02f4840d-74f0-470f-96ed-86d86a48ff38`: created, taken, buyer invoice `3327cca6…`, hold `65e91e73…` paid by Alice, fiat sent, released; relay published `success`; both clients showed completed | None. Hold settled once on Alice's node (1005 sats), payout settled once on Bob's node (995 sats), zero fees. Order resolved, no retained storage. Harness `c39881e`, app `0ed7efb`, mostrod 0.18.7 |
| `20260908T184853Z-a78aaa152518` | Linux | happy_sell | Both apps showed the order `active` after Alice paid the hold | The narrative waited for a public relay event with status `active`, which the daemon never publishes: the book keeps `in-progress` from the take until the order closes and tells only the parties about `active`. The fake daemons in tests published it, hiding the mismatch. Fixed in the following commit. Cleanup cancelled the order through both clients with relay confirmation; hold payment FAILED, no settlement, profiles removed |
| `20260908T171517Z-a54d8617c8eb` | Linux | happy_sell | Order created and taken, buyer invoice provided, hold invoice paid by Alice | Post-payment lookup of Alice's outgoing hash used `lookup_invoice`; LND answered `Unknown: there are no existing invoices` and only `NotFound` was treated as a miss. Fixed in `8de9f4e`. Cleanup then trusted Alice's screen, which app v2 marks `cancelled` on her request alone, while the daemon had only opened a cooperative cancel and held her 1005 sats for over an hour. Fixed in `aaa0615` and `869644a` (relay-confirmed closure, counterpart escalation). Recovery also hit a relay-list draw race, fixed in `30c3298`. Resolved by `cleanup` run `20260908T184549Z-220e080dd721`: relay `canceled`, hold payment FAILED, profiles removed |
| `20260908T161350Z-259115af1b1c` | Linux | happy_sell | Order created and taken | Bob landed on the buyer invoice form while the adapter waited for an order-detail status node. Fixed in harness `e846715` and app `0ed7efb` |
| `20260908T161029Z-86b51b35d10b` | Linux | happy_sell | Bootstrap | Native identifier parser rejected relay IDs containing `ws://`. Fixed in `dd77f75` |
| `20260908T160318Z-e617fd03c704` | Linux | happy_sell | Order created | Bob could not locate the merged native book-tab label. Fixed in `b6bec7c` |

## Known app v2 product issues

### Web build did not persist trades (fixed locally 2026-09-09)

Two independent causes, both fixed on the app's `feat/mortsom-linux-accessibility` branch, upstream PR
MostroP2P/app#408: the `wasm32` storage backend stubbed every trade operation (`fe7fd55` adds the `trades` and
`trade_keys` IndexedDB stores, part of app issue #233), and the Dart bootstrap skipped `init_db` on the web
altogether (`def4d40`), which also meant the "implemented" web chat store had never been opened.

### Optimistic cancel label

`cancel_order` in the app's Rust core (`rust/src/api/orders.rs`) marks the trade `Canceled` in the local
database as an optimistic update regardless of the order's status. For an `active` or `fiat-sent` order the
daemon only opens a cooperative cancel and keeps the seller's hold, so the requesting user sees "cancelled"
while their sats are still escrowed. The app's own FSM (`rust/src/mostro/fsm.rs`) already says the status
must not change on a cooperative cancel request. Mortsom no longer trusts that label; the app should apply
the FSM transition instead of a blanket `Canceled`. To be filed against `MostroP2P/app`.

## Recording a pass

Add the run ID, platform, scenario, the order ID, both settled payment hashes (redacted to a prefix) and the
artifact path. Change the cell to **Accepted** only when `summary.json` reports the scenario passed and
`cleanup.json` reports every order resolved and no retained storage.
