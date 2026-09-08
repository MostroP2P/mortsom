# Ortsom UI parity acceptance ledger

The normative plan and task checklists are in [spec.md](../spec.md), sections
10 and 17. Source definitions are in `../ortsom/docs/scenarios.md` and
`../ortsom/crates/ortsom/src/scenarios/`. This ledger records actual acceptance,
not registration or unit-test coverage.

A source row counts once only after a live v2 UI run proves its required
UI/relay/wallet assertions and completes cleanup. Linux and Web results are
separate. Skips, fake tests, smoke launches and duplicate aliases never count.

**Current accepted totals: Linux 0/20; Web 0/20.**

| # | Source scenario | Source file | Linux | Web |
|---|---|---|---|---|
| 1 | happy_sell | happy.rs | Failed; input not accepted | Failed; offscreen input |
| 2 | happy_buy | happy.rs | Pending | Pending |
| 3 | cancel_before_taken | cancellation.rs | Pending | Pending |
| 4 | cooperative_cancel | cancellation.rs | Pending | Pending |
| 5 | unilateral_cancel_rejected | cancellation.rs | Pending | Pending |
| 6 | cancel_after_take_unpaid | cancellation.rs | Pending | Pending |
| 7 | pending_order_expires | expiration.rs | Pending | Pending |
| 8 | hold_invoice_expires | expiration.rs | Pending | Pending |
| 9 | buyer_invoice_deadline | expiration.rs | Pending | Pending |
| 10 | full_trade_with_rating | happy.rs | Pending | Pending |
| 11 | range_order | happy.rs | Pending | Pending |
| 12 | market_price_order | happy.rs | Pending | Pending |
| 13 | dispute_by_buyer | dispute.rs | Pending | Pending |
| 14 | dispute_by_seller | dispute.rs | Pending | Pending |
| 15 | dispute_then_cooperative_cancel | dispute.rs | Pending | Pending |
| 16 | invalid_buyer_invoice | payment_failure.rs | Pending | Pending |
| 17 | payout_invoice_below_the_expiry_floor | payment_failure.rs | Pending | Pending |
| 18 | payment_fails_on_release | payment_failure.rs | Pending | Pending |
| 19 | buyer_invoice_expires_mid_flight | payment_failure.rs | Pending | Pending |
| 20 | payout_slots_capped_per_destination | payment_failure.rs | Pending | Pending |

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
