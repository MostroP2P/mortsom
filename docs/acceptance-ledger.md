# Acceptance ledger

One row per scenario and platform. A cell is **accepted** only after a live run through the real app v2 UI
proves the assertions in [spec.md](../spec.md) section 2 and finishes verified cleanup. Builds, unit tests,
onboarding, identity probes, cancellation diagnostics and skipped cases never count.

**Accepted: Linux 0/2, Web 0/2.**

| Scenario | Linux | Web |
|---|---|---|
| `happy_sell` | Not accepted. Four live attempts; the latest reached the hold-invoice payment (see below) | Not accepted. No live attempt since the rebuilt clients |
| `happy_buy` | Not accepted. No live attempt | Not accepted. No live attempt |

## Live attempts

Newest first. Every attempt was cleaned up with no settled payment; all four orders were cancelled through
the UI. Run artifacts are under gitignored `artifacts/<run-id>/`.

| Run | Platform | Scenario | Reached | Failure |
|---|---|---|---|---|
| `20260908T171517Z-a54d8617c8eb` | Linux | happy_sell | Order created and taken, buyer invoice provided, hold invoice paid by Alice | Post-payment lookup of Alice's outgoing hash used `lookup_invoice`; LND answered `Unknown: there are no existing invoices` because Alice had never issued an invoice, and the code treated only `NotFound` as "not an incoming invoice". Profiles retained; `cleanup` pending |
| `20260908T161350Z-259115af1b1c` | Linux | happy_sell | Order created and taken | Bob landed on the buyer invoice form while the adapter waited for an order-detail status node. Fixed in harness `e846715` and app `0ed7efb` |
| `20260908T161029Z-86b51b35d10b` | Linux | happy_sell | Bootstrap | Native identifier parser rejected relay IDs containing `ws://`. Fixed in `dd77f75` |
| `20260908T160318Z-e617fd03c704` | Linux | happy_sell | Order created | Bob could not locate the merged native book-tab label. Fixed in `b6bec7c` |

## Recording a pass

Add the run ID, platform, scenario, the order ID, both settled payment hashes (redacted to a prefix) and the
artifact path. Change the cell to **Accepted** only when `summary.json` reports the scenario passed and
`cleanup.json` reports every order resolved and no retained storage.
