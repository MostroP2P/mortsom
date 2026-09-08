# Ortsom UI parity acceptance ledger

The normative scenario rules are in [spec.md](../spec.md), section 8; the historical
task checklists and twenty-scenario catalogue are in [docs/history/spec-v0.3.md](history/spec-v0.3.md). Source definitions are in `../ortsom/docs/scenarios.md` and
`../ortsom/crates/ortsom/src/scenarios/`. This ledger records actual acceptance,
not registration or unit-test coverage.

A source row counts once only after a live v2 UI run proves its required
UI/relay/wallet assertions and completes cleanup. Linux and Web results are
separate. Skips, fake tests, smoke launches and duplicate aliases never count.

**Current checkpoint: happy_sell and happy_buy only. Accepted totals: Linux 0/2; Web 0/2.**

The operator reduced the target on 2026-09-08. Stop after those four cells pass with verified cleanup and
present the work for review. Rows 3–20 and resource benchmark repetitions are deferred.

| # | Source scenario | Source file | Linux | Web |
|---|---|---|---|---|
| 1 | happy_sell | happy.rs | Failed; input not accepted | Failed; offscreen input |
| 2 | happy_buy | happy.rs | Pending | Pending |
| 3 | cancel_before_taken | cancellation.rs | Deferred | Deferred |
| 4 | cooperative_cancel | cancellation.rs | Deferred | Deferred |
| 5 | unilateral_cancel_rejected | cancellation.rs | Deferred | Deferred |
| 6 | cancel_after_take_unpaid | cancellation.rs | Deferred | Deferred |
| 7 | pending_order_expires | expiration.rs | Deferred | Deferred |
| 8 | hold_invoice_expires | expiration.rs | Deferred | Deferred |
| 9 | buyer_invoice_deadline | expiration.rs | Deferred | Deferred |
| 10 | full_trade_with_rating | happy.rs | Deferred | Deferred |
| 11 | range_order | happy.rs | Deferred | Deferred |
| 12 | market_price_order | happy.rs | Deferred | Deferred |
| 13 | dispute_by_buyer | dispute.rs | Deferred | Deferred |
| 14 | dispute_by_seller | dispute.rs | Deferred | Deferred |
| 15 | dispute_then_cooperative_cancel | dispute.rs | Deferred | Deferred |
| 16 | invalid_buyer_invoice | payment_failure.rs | Deferred | Deferred |
| 17 | payout_invoice_below_the_expiry_floor | payment_failure.rs | Deferred | Deferred |
| 18 | payment_fails_on_release | payment_failure.rs | Deferred | Deferred |
| 19 | buyer_invoice_expires_mid_flight | payment_failure.rs | Deferred | Deferred |
| 20 | payout_slots_capped_per_destination | payment_failure.rs | Deferred | Deferred |

`gated_payouts_do_not_starve_other_destinations` is an additional follow-up.
The detailed reason for prioritizing pending-order expiry instead is in spec
section 10. Unsupported protocol-malformation and solver-only cases cannot
substitute for missing app journeys.

## Verified prerequisites

- Native Linux release build and public accessibility smoke passed with
  separate Xvfb, DBus, AT-SPI and Secret Service runtime.
- Web release/WASM build and public accessibility smoke passed using Chromium,
  an isolated profile and a loopback server with COOP/COEP headers.
- Linux run `20260908T100217Z-a9eaf0273a29`: both actors passed launch,
  onboarding, node/relay settings, restart and identity persistence checks.
- Web run `20260908T100434Z-7f107aaadcd8`: the same bootstrap checks passed.
- Both cited runs reported successful client teardown, but startup-failure
  cleanup coverage and independent process/mount leak auditing remain pending.

## Failed live acceptance attempts

| Run | Platform | Source | Observed failure | Required next validation |
|---|---|---|---|---|
| 20260908T100217Z-a9eaf0273a29 | Linux | happy_sell | Confirmation never appeared; screenshot shows empty amount despite successful accessibility text calls | Focus/edit and public value readback before submission |
| 20260908T100434Z-7f107aaadcd8 | Web | happy_sell | Sats input exists in fixed mode but remains non-interactable beneath the viewport | Public scrolling, fresh element lookup and bounded interactability |

Artifacts remain under ignored `artifacts/<run-id>/`. These attempts did not
reach payout settlement and are not successful trade replications.

## Recording a future pass

For each accepted cell, record the exact command, run ID, app revision/artifact
digest, running daemon digest, declared limits, correlated evidence locations
and cleanup result. Include any retained failed attempts; a later clean run
must not erase their diagnosis. Advance a specification task only when its
stated verification is satisfied.

Linux checkpoint attempt `20260908T160318Z-e617fd03c704` created sell order
`911fcef1-b3eb-497b-8018-3f3c2d97071d` and observed pending, then failed on Bob's missing native
`order.book.tab.buy` selector. No payments or invoices occurred. UI cancellation and complete client/profile
cleanup passed; the persistent stack survived. This is not an accepted happy_sell run.

Linux checkpoint retry `20260908T161350Z-259115af1b1c` progressed through creation and take of sell
`11284250-f982-43ba-8c2b-33aadbede145`. Bob's public invoice form showed the exact order ID and 995 sats,
but the adapter expected an order-detail status node and timed out. No invoice/payment was issued.
UI cancellation, both client shutdowns and profile removal passed; happy_sell remains unaccepted.

At the pause checkpoint, commits `e846715` (harness) and `0ed7efb` (app) correct direct take-to-invoice routes
with exact order binding. Focused tests/review passed; both app targets require rebuilding before live retry.
No fourth run was started and no acceptance cell changed. See [HANDOFF.md](HANDOFF.md).
