# mostro-cli contract

What the `cli` profile (spec §6.3) relies on in mostro-cli's machine interface. A change to anything here
is a contract change and needs a coordinated review of mostro-cli and Mortsom, as a change to an app
automation identifier does. mostro-cli publishes its own source of truth: the generated schemas in its
`docs/schemas/` and the offline discovery (`mostro-cli --output json schema`).

## Invocation

- Always `--output json`. Node commands also carry `--mostro-pubkey <hex>`, `--relays ws://127.0.0.1:<port>`
  and `--transport nip44`, and `--reply-timeout 60` when discovery lists that global option
  (`data.global_options[].name`); the CLI's own default is 15 s, short for a daemon slowed by its database.
  Writes carry `--yes`; the machine interface never prompts.
- Environment: only `MCLI_DIR=<actor>/mcli`, `HOME=<actor>/home` (and nothing else); working directory
  `<actor>/home`. One invocation at a time per actor.
- Invoices: `--invoice -` and the invoice on standard input, one line.
- One outcome document on stdout:
  `{"schema_version": "1", "command": "<dotted name>", "status": "ok"|"error"|"uncertain", "data": ..., "error": {"code", "message", "reason"?}, "warnings": [...], "meta": {...}}`.
  Exit 0 with `ok`, 1 or 2 otherwise. Anything else is an automation-contract failure.
- `uncertain` is never retried.

## Error codes and failure categories

| Codes | Category |
|---|---|
| `invalid_arguments`, `output_not_supported`, `protocol_unsupported` | automation-contract |
| `relay_connection_failed`, `relay_read_failed`, `read_timeout`, `read_limit_exceeded`, `reply_timeout`, `request_rejected`, `pow_required`; any other `uncertain` | relay |
| `configuration_missing`, `configuration_invalid`, `identity_missing`, `identity_unreadable`, `identity_exists`, `store_unwritable`, `store_unreadable`, `order_not_saved` | environment-misconfiguration |
| everything else (`node_refused`, `order_declined`, `order_unknown`, `reply_malformed`, `internal_error`, ...) | product |

`order_not_saved` (from `order new`, `order take-sell`, `order take-buy` and `order sync`) is `uncertain`
because the node acted, but its cause is the actor's store, which could not record it: it is classified
like `store_unwritable`, not as a relay failure, and like every `uncertain` it is never retried.

A process that cannot start, outlives its deadline (90 s, longer than the CLI's own connection, reply and
cleanup deadlines put together, after which it is killed and reaped) or is ended by a signal is
test-infrastructure. A process that exits non-zero without printing a document is a crash of the system
under test (product), except exit 2 with no document, which is the argument parser refusing what Mortsom
sent (automation-contract). Standard error and any parser complaint are scrubbed of Lightning invoices
before they are kept.

An `invalid_arguments` from `order add-invoice` counts as the CLI refusing the invoice only with
`reason: "invalid_invoice"`; any other argument refusal is an automation-contract failure.

## Commands

| Adapter action | Command | Fields read |
|---|---|---|
| discovery (before actors start) | `schema` | `data.executable_version`, `data.commands[].name`, `.output_modes`, `.input_schema.properties` (`order.new` → `expires-in`) |
| onboarding | `identity init` | `data.identity_pubkey` |
| displayed / existing identity | `identity show` | `data.identity_pubkey` |
| node and relay check | `order list` | `data.node_pubkey` (must be the daemon under test), `data.orders` |
| own orders | `order mine` | `data.orders[].id` |
| create | `order new --side --currency --fiat-amount [--fiat-amount-max] --payment-method --sats [--premium] [--expires-in] --yes` | `data.order.id`, `data.order.status`, `data.next_step`, `data.payment_request` (bond) |
| take | `order take-sell` / `order take-buy --order [--amount] --yes` | `data.next_step` (`add_invoice`, `pay_hold_invoice`, `pay_bond_invoice`), `data.payment_request`, `data.order.sats_amount` (payout to invoice) |
| state | `order info --order` | `data.orders[]` with `id` and `status` (mostro-core status names) |
| node requests | `message list --order --since <minutes>` | `data.messages[]`: `action`, `next_step` (`pay_hold_invoice`, `add_invoice`, `pay_bond_invoice`, `add_bond_invoice`), `created_at_unix`, `payload.invoice`, `payload.amount`, `payload.order.sats_amount`; `data.truncated` |
| buyer invoice | `order add-invoice --order --invoice - --yes` | refusals: `node_refused` + `reason`, `invalid_arguments` |
| fiat sent / release | `order fiat-sent` / `order release --order --yes` | status only |
| cancel | `order cancel --order --yes` | `data.result`; refusals `node_refused` + `reason` (`order_already_canceled`, `not_allowed_by_status`, `is_not_your_order`), `order_unknown` |
| rate | `order rate --order --rating --yes` | status only |
| dispute | `dispute open --order --yes` | status only |
| bond claim | `bond add-invoice --order --invoice - --yes` | status only; the claim reads `pending` until `bond-payout-completed` |
| range children | `order sync` | (listed for the range capability) |

Actions read from `message list`: `bond-slashed` (cause `dispute` when the order shows
`dispute-initiated-by-you`, `dispute-initiated-by-peer`, `admin-took-dispute`, `admin-settled` or
`admin-canceled`, else `timeout`), `bond-payout-completed`.

The `message list` window is `--since 1440` during a run. Recovery reads from the run's start, recorded
in the journal, to now, capped at 10080 minutes (the CLI's maximum). The daemon asks for a payout
replacement once, after its retries are spent, so a request older than that window cannot be read: an
order whose `order info` still waits on the actor (a payout or a bond-claim invoice) is left unresolved
with that reason, never reported as cleaned up.

With `data.truncated` true the list may be incomplete (the relay returned its per-relay limit, or more
than the record cap matched), so an absent action is inconclusive: a request, `bond-slashed` or
`bond-payout-completed` missing from it proves nothing. A wait keeps polling within its deadline; a check
that needs the absence fails as a relay failure instead of passing.

## Required and optional commands

Required (the run refuses a CLI without them): `identity.init`, `identity.show`, `order.list`, `order.new`,
`order.info`, `order.take-sell`, `order.take-buy`, `order.add-invoice`, `order.fiat-sent`, `order.release`,
`order.cancel`, `message.list`; and `order.add-invoice`'s `invoice` input must carry `"x-stdin": "-"`
(a CLI that only takes the invoice as an argument would put it in the process arguments).

Optional, each enabling a capability: `order.mine` (with `order.new` and `order.info`: `create-order`),
`order.sync` (`range-order`), `order.rate` (`rating`), `dispute.open` (`dispute`), `bond.add-invoice`
(`bond`), `expires-in` on `order.new` (`order-expiry`).
