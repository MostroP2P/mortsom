# Acceptance ledger

One row per scenario and platform. A cell is **accepted** only after a live run through the real app v2 UI
proves the assertions in [spec.md](../spec.md) section 2 and finishes verified cleanup. Builds, unit tests,
onboarding, identity probes, cancellation diagnostics and skipped cases never count.

**Accepted: Linux 1/2, Web 0/2.**

| Scenario | Linux | Web |
|---|---|---|
| `happy_sell` | **Accepted** 2026-09-08, run `20260908T190151Z-d027fc8a5528` (see below) | Not accepted. No live attempt since the rebuilt clients |
| `happy_buy` | Not accepted. No live attempt | Not accepted. No live attempt |

## Live attempts

Newest first. Run artifacts are under gitignored `artifacts/<run-id>/`.

| Run | Platform | Scenario | Reached | Failure |
|---|---|---|---|---|
| `20260908T190151Z-d027fc8a5528` | Linux | happy_sell | **Passed.** Order `02f4840d-74f0-470f-96ed-86d86a48ff38`: created, taken, buyer invoice `3327cca6…`, hold `65e91e73…` paid by Alice, fiat sent, released; relay published `success`; both clients showed completed | None. Hold settled once on Alice's node (1005 sats), payout settled once on Bob's node (995 sats), zero fees. Order resolved, no retained storage. Harness `c39881e`, app `0ed7efb`, mostrod 0.18.7 |
| `20260908T184853Z-a78aaa152518` | Linux | happy_sell | Both apps showed the order `active` after Alice paid the hold | The narrative waited for a public relay event with status `active`, which the daemon never publishes: the book keeps `in-progress` from the take until the order closes and tells only the parties about `active`. The fake daemons in tests published it, hiding the mismatch. Fixed in the following commit. Cleanup cancelled the order through both clients with relay confirmation; hold payment FAILED, no settlement, profiles removed |
| `20260908T171517Z-a54d8617c8eb` | Linux | happy_sell | Order created and taken, buyer invoice provided, hold invoice paid by Alice | Post-payment lookup of Alice's outgoing hash used `lookup_invoice`; LND answered `Unknown: there are no existing invoices` and only `NotFound` was treated as a miss. Fixed in `8de9f4e`. Cleanup then trusted Alice's screen, which app v2 marks `cancelled` on her request alone, while the daemon had only opened a cooperative cancel and held her 1005 sats for over an hour. Fixed in `aaa0615` and `869644a` (relay-confirmed closure, counterpart escalation). Recovery also hit a relay-list draw race, fixed in `30c3298`. Resolved by `cleanup` run `20260908T184549Z-220e080dd721`: relay `canceled`, hold payment FAILED, profiles removed |
| `20260908T161350Z-259115af1b1c` | Linux | happy_sell | Order created and taken | Bob landed on the buyer invoice form while the adapter waited for an order-detail status node. Fixed in harness `e846715` and app `0ed7efb` |
| `20260908T161029Z-86b51b35d10b` | Linux | happy_sell | Bootstrap | Native identifier parser rejected relay IDs containing `ws://`. Fixed in `dd77f75` |
| `20260908T160318Z-e617fd03c704` | Linux | happy_sell | Order created | Bob could not locate the merged native book-tab label. Fixed in `b6bec7c` |

## Known app v2 product issue

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
