# `mobilev2` inventory and automation contract

Phase 7.1 of `spec.md`: what the second-generation application
(`../mobilev2`, package `foundation.mostro.app`, Flutter + Rust core through
`flutter_rust_bridge`, `mostro-core 0.14.x` inside the app) exposes for each
capability of the shared `AppAdapter` contract, and what it must add for
Mortsom to drive it. Mortsom never calls the app's Rust core; it only uses
the UI and public Android behaviour, exactly as with the classic app.

## Inventory (as of `mobilev2` `main`, 7625b24)

| Capability | v2 surface | Notes |
|---|---|---|
| Onboarding | `/walkthrough` (three slides, Skip/Done); identity generated on first launch; `/keyManagement` shows/imports keys | No community/node step in the walkthrough. |
| Custom node selection | Settings → `MostroNodeSelector` dialog: pubkey field, *Use default*, *Confirm*; default `82fa8c…88390` in `lib/core/mostro_defaults.dart` | Same daemon-pubkey model as the classic app. |
| Local relay configuration | Settings → `/relays` (`relay_management_card.dart`): add/delete relays; defaults `wss://relay.mostro.network`, `wss://nos.lol` | Test builds must seed `MORTSOM_RELAYS` and disable production fallback (spec §17.3). |
| NWC | Settings → `/walletSettings` → `/connectWallet` (paste or scan URI) ; `nwc_provider.dart`; payment via `nwc_payment_widget.dart`, invoice generation via `nwc_invoice_widget.dart` | Same NWC flow as the classic app. |
| Create order | `/addOrder` (`add_order_screen.dart`), currency-aware payment methods, fixed/market price | |
| Take order | `/takeSell`, `/takeBuy` (`take_order_screen.dart`, amount only for range orders) | |
| Hold invoice payment | `/payInvoice` (`pay_lightning_invoice_screen.dart`, NWC or copy) | Cancel from the pay screen confirms in a dialog. |
| Buyer invoice | `/addInvoice` (`add_lightning_invoice_screen.dart`, NWC generate or manual) | |
| Fiat sent / release / cancel / dispute / rate | `/tradeDetail` (`trade_detail_screen.dart`) with `TradeStatus` = loading, pending, waitingInvoice, waitingPayment, inProgress, active, fiatSent, completed, cancelled, disputed, pendingRating, rated | `pendingRating`/`rated` are post-success UI states, mapped to `Completed` by the adapter. |
| Notifications / deep links | present (`/notifications`, `firebase.json`) | Not part of the v2 contract yet; scenarios needing them are skipped with a reason. |
| Test-environment marker | `env.marker`, shown by `lib/main_mortsom.dart` builds | Adopted (below). |
| Accessibility identifiers | `lib/core/automation/automation_ids.dart` | Adopted (below). |

## Application-facing changes for v2 (spec §17)

The v2 app adopted the same mechanism the classic app carries (its
PR #650); the pieces are:

1. `lib/core/automation/automation_ids.dart` with the identifiers listed in
   `crates/app-adapters/mobile-v2/src/selectors.rs` (`Ids::ALL` plus the
   dynamic prefixes `settings.relays.item.$`, `order.book.item.$`,
   `trades.item.$`, `order.create.currency.$`), applied through an
   `AutomationId` widget (`MergeSemantics(Semantics(identifier, container:
   true))`), so they surface as Android `resource-id`s.
2. `lib/main_mortsom.dart` + `--dart-define=MORTSOM_TEST_ENV=true`: arms the
   test environment, seeds `MORTSOM_RELAYS`, disables the production relay
   fallback, allows `ws://`, and shows the `env.marker` banner.
3. `order.status` exposes the machine name of `TradeStatus` (kebab-case) as
   its accessibility label; `keys.public_key`, `settings.mostro_node.pubkey`,
   `wallet.connection`, `pay.invoice.text` and `invoice.nwc.text` expose
   their values the same way.

The adapter's contract test `selectors_match_the_v2_app_contract` reads the
app's registry out of the checkout and fails when the two drift apart. An
APK built from a checkout without the contract still fails at onboarding
with "test-environment marker missing", which is the honest result.

## Where v2 differs from the classic app

The vocabulary is shared, but three flows are not, and the adapter follows
the app rather than the other way round:

* **The buyer invoice has no confirm step.** With a wallet connected, v2
  asks NWC for the invoice and submits it in one action, so there is no
  `invoice.nwc.generate`/`invoice.nwc.confirm` pair. `invoice.nwc.text` is a
  readout of what was submitted. The manual form appearing instead means NWC
  produced nothing, which the adapter reports rather than works around.
* **The pay screen offers Cancel only on the manual path.** With a wallet
  connected the auto-pay widget covers it, so `cancel_payment` falls back to
  cancelling from the trade detail. Both reach the same dialog.
* **Settings → Wallet leads straight to the connect screen** while no wallet
  is configured, so `wallet.settings.connect` is tapped only when present.

Two further notes that are contract, not difference:

* A pending order the user created opens on `/my_order`, not
  `/trade_detail`, so both screens expose `order.status` and `order.id`.
* The book tabs are named by their visible label and each lists the opposite
  side — the "Buy BTC" tab lists sell orders. The adapter picks the tab that
  lists the side it wants. This matches the classic app, whose adapter had
  the pairing inverted and is fixed alongside.

## Shared contract changes (Phase 7.2)

None were needed: the normalized states and capabilities already cover v2.
`pending-rating`/`rated` and `loading` are v2 renderings handled inside the
adapter (`states.rs`), not new business states.

## Parity (Phase 7.5)

Scenarios tagged `parity` (`happy_sell`, `restart_at_taken`,
`cancel_pending`, `cooperative_cancel`, `unauthorized_actions_at_taken`)
require only capabilities both profiles declare. Run them per profile in
isolated runs and compare:

```sh
mortsom run --tag parity                      # mobile.profile = mobile-legacy
mortsom --config config/mobile-v2.example.toml run --tag parity
mortsom parity <legacy-run-id> <v2-run-id> --intentional config/parity-intentional.toml
```

`mortsom parity` compares, per scenario, outcome, per-actor state path,
relay path, wallet settlement, cleanup of the orders that scenario created
and automation-contract failures (read from each run's `timeline.json`). Every
difference is a finding unless `config/parity-intentional.toml` names it
with a reason.
