# Mortsom specification

**Version:** 1.0

**Updated:** 2026-09-08

**Status:** normative for all new work

**Target app:** Mostro app v2 (`MostroP2P/app`, local checkout `../mobilev2`)

**Default client platform:** Linux

This document says what Mortsom is and how it must behave. It does not record progress, attempts or task
checklists; those live in the documents listed in section 15. The previous specification, with its history
and attempt ledger, is archived at [docs/history/spec-v0.3.md](docs/history/spec-v0.3.md).

Everything in this repository is written in English.

## 1. Motivation

Mortsom drives two real Mostro clients, Alice and Bob, through a complete peer-to-peer trade against a local
regtest stack, and proves the trade from the outside: what each user saw, what the daemon published and what
each Lightning wallet actually paid or received.

Its first version did this with Android emulators. Two things changed:

- **Emulators are expensive.** Two Android emulators plus the stack saturate an ordinary development machine.
  Lighter client platforms are needed before the scenario catalogue can grow.
- **The client is changing.** The mobile app v1 (`MostroP2P/mobile`) is being deprecated. Its replacement is
  app v2 (`MostroP2P/app`), a Flutter application with a Rust core that already builds for Linux and Web.

Mortsom therefore targets app v2 and adds two execution modes, **Linux** and **Web**, both produced from the
same Flutter source. Android remains a selectable option so the emulator path is not lost, but no new Android
work happens until the CPU and memory cost of Linux and Web has been observed.

## 2. Goal and acceptance

Mortsom is complete for this version when an operator can:

1. Bring up and inspect the regtest stack with `mortsom stack up` and `mortsom stack status`.
2. Check prerequisites for a chosen platform with `mortsom doctor` without creating an order or paying anything.
3. Run `happy_sell` and `happy_buy` with Alice and Bob on Linux, and again on Web, and obtain **four accepted
   cells** with correlated order, escrow and payout evidence and verified cleanup.
4. Recover any failed run with `mortsom cleanup <run-id>` without inventing identities or losing funds.
5. Run every scenario of the happy-path and cancellation families (section 8) on Linux and on Web, one
   accepted cell per scenario and platform, selected one by one or by family tag.

More families follow once these cells are accepted. A scenario, platform or benchmark outside this list
needs an explicit operator instruction.

An accepted cell requires all of the following. Nothing weaker counts:

- Both clients reached the scenario's terminal state through their normal UI.
- The visible order ID matches the daemon-signed public relay events for that order.
- The seller's hold invoice and the buyer's payout invoice each settled exactly once on the actors' wallets,
  with the expected amounts and fees.
- Every order created by the run is resolved, both client sessions are closed, and owned processes,
  displays, buses, servers and profiles are gone.

Builds, unit tests, onboarding, identity probes, cancellation diagnostics and skipped cases are useful
evidence but are never counted as an accepted cell.

## 3. Actors and roles

| Actor | Role | Wallet |
|---|---|---|
| Alice | App v2 user. Seller in `happy_sell`, taker-seller in `happy_buy` | LND `alice` |
| Bob | App v2 user. Buyer in `happy_sell`, maker-buyer in `happy_buy` | LND `bob` |
| Carol | The server side: the LND node behind `mostrod` | LND `carol` |

Alice and Bob each run their own isolated copy of app v2. Their wallets are external LND nodes in the stack,
operated by Mortsom in the role of the user's wallet: it pays the invoice the app shows and produces the
invoice the user types into the app. Wallet actions never publish protocol messages.

Both actors run on the same platform in one invocation. Mixed pairs are out of scope.

## 4. Operator workflow

```sh
# Regtest environment owned by Mortsom.
mortsom stack up
mortsom stack status
mortsom stack down

# Prerequisites for a platform. No order, no payment.
mortsom doctor --platform linux
mortsom doctor --platform web

# Scenarios. Linux is the default platform.
mortsom run happy_sell
mortsom run happy_sell --platform web
mortsom run happy_buy --platform linux
mortsom run happy_buy --platform web

# Reuse a build instead of compiling from source.
mortsom run happy_sell --platform linux --app-artifact /path/to/bundle/mostro
mortsom run happy_sell --platform web   --app-artifact /path/to/build/web

# Frozen option: requires an explicit Android-compatible configuration.
mortsom --config android.local.toml run happy_sell --platform android

# Recover a run that left orders, payments or profiles behind.
mortsom cleanup <run-id>
```

`run` attaches to an already ready stack and fails with instructions when it is absent. It never creates or
destroys the stack. One invocation runs one scenario with fresh actors. Unknown platforms fail before anything
is built or launched. Selecting Android must neither silently use Linux nor claim capabilities it lacks.

## 5. Stack

Mortsom owns its regtest environment and needs no other tool to create it. `mortsom stack up` builds the
whole world from nothing on a machine with Docker: a regtest chain, three Lightning nodes with balanced
channels, a Nostr relay and a `mostrod` with an identity minted for this stack. `stack status` reports it and
`stack down` removes it, volumes included.

| Service | Role | Host port |
|---|---|---|
| `bitcoind` | Regtest chain; mines its own coins, so nothing needs external funding | none |
| `lnd-alice` | Alice's wallet | 10001 |
| `lnd-bob` | Bob's wallet | 10002 |
| `lnd-carol` | The daemon's escrow node | 10003 |
| `relay` | In-memory Nostr relay reachable only from this host | 7000 |
| `mostro` | The daemon under test | none |

Topology: Alice and Bob each open one channel to Carol, funded at 1,000,000 sats with 500,000 pushed across,
so both have outbound liquidity to pay a hold invoice and inbound liquidity to receive a payout. There is
deliberately no Alice-to-Bob channel: every payment has the daemon's node on one end, and a second route would
only let a payment succeed by accident. Carol exists so the daemon never escrows on a node an actor pays from.

`stack up` must:

1. Resolve the daemon image. The default is a pinned release recorded in configuration; `--mostro-image`
   selects another. Pull it; a failed pull is an error, never permission to use a stale cached image.
2. Start the chain, nodes and relay; wait for each to be usable; fund the nodes and open the channels above,
   skipping work already done so the command is re-runnable against a half-built stack.
3. Mint the daemon identity for this stack, write its settings with the declared test limits, and start the
   daemon as the invoking user so it can read owner-only files. Verify the running container matches the
   pulled digest.
4. Wait until the daemon publishes its info event on the relay, then validate regtest networks, advertised
   relay endpoints, channel liquidity and declared limits.
5. Write the public connection references, daemon public identity, image digest and stack fingerprint to
   Mortsom's ignored local overlay. Node credentials and the daemon's private key stay in owner-only files
   that are never echoed, logged or copied into artifacts.

`stack status` exits non-zero until the stack is usable, so it can be waited on. It reports each service,
each node's active channels and the daemon identity in effect.

`stack down` stops the services, removes their volumes and deletes the generated identity and credentials,
so no public key is left that nothing answers on. Both `up` and `down` refuse to touch containers or volumes
that this Mortsom checkout did not create.

`run` never creates, updates or destroys the stack. Scenario teardown never removes containers or volumes. A
stack reset is an explicit operator action; it is not evidence that an order was cancelled or a payment
refunded. The published ports collide with a local Polar network or relay; those must be stopped first.

## 6. Platforms

### 6.1 Linux (default)

The app is built with `flutter build linux` from the Mortsom entrypoint and driven through AT-SPI.

Each actor owns a native app process, a virtual X display, a private session bus, an AT-SPI bus, a Secret
Service instance and separate XDG directories. Sharing any of these can leak one actor's identity into the
other. Readiness waits prove accessibility registration and an unlocked credential collection before the app
starts. Displays are allocated without fixed-port races. Incidental desktop services that outlive the run are
disabled.

Flutter's Linux engine does not export `Semantics.identifier`. In Mortsom builds the app exposes
`[mortsom:<identifier>]<readout>` as the accessible name; this namespace stays out of production labels.
Mortsom uses only public AT-SPI interfaces for actions, focus, text entry, scrolling and state. A successful
input call is not proof that the app accepted the value.

### 6.2 Web

The app is built with `flutter build web`, including its Rust/WASM bundle, and driven through W3C WebDriver.

Mortsom serves the bundle on loopback with `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`, limits requests to the bundle root, and owns every request task.
Alice and Bob use separate browser profiles with the sandbox enabled. Elements are located by exact
`flt-semantics-identifier` values with escaped selectors; only public DOM and accessibility interactions are
allowed. No JavaScript calls into Dart, Rust or application state.

Flutter may rebuild offscreen semantics after focus or scroll: re-find elements after transitions, scroll
through user actions, and wait for interactability with bounded predicates instead of fixed sleeps.

### 6.3 Android (frozen)

The emulator-based path from the first version remains selectable. It receives no new features in this
version. Its operating instructions are in [docs/history/legacy-android.md](docs/history/legacy-android.md).

## 7. Client builds and automation contract

Mortsom builds app v2 from `lib/main_mortsom.dart` with a test-environment marker, the daemon's public
identity and the local relay seeds. Production relay fallback is disabled before any subscription starts.
Advertised relay addresses are validated from the client's own network namespace.

Builds follow the app's supported scripts. Flutter Rust Bridge bindings are regenerated with
`scripts/frb-generate.sh` before a source build; generated bindings are never edited by hand. Linux and Web
builds run sequentially because binding generation writes shared source files.

Mortsom records the source revision, dirty flag, build flags, tool versions and the bundle digest. Both actors
use the same artifact. A prebuilt artifact must pass the same visible environment and identity checks as a
fresh build.

The app owns the automation contract (`docs/automation-contract.md` in the app repository): stable identifiers
for onboarding, navigation, settings, form fields, order identity and status, invoices, cancellation, errors,
and later disputes and ratings. Missing controls are added as ordinary accessible product UI with widget
tests, never as test-only business endpoints.

## 8. Scenarios

Scenarios express business actions through a platform-neutral app adapter. Linux and Web run the same
narrative; scenario bodies contain no AT-SPI or WebDriver detail.

### 8.1 Families and tags

The catalogue is organised in families. Each scenario carries its family name as a tag, and `--tag` runs
a whole family; `smoke` cuts across families to mark the cheap subset (`happy_sell`, `happy_buy` and
`cancel_before_taken`). `mortsom list` prints every scenario with its tags and requirements.

| Family | Tag | What it covers |
|---|---|---|
| Happy path | `happy-path` | Trades that work: sell, buy, range (sold and bought), market price, rating |
| Cancellations | `cancellation` | Orders ended before the sats move — unilateral, abandoned, cooperative |
| Expirations | `expiration` | The daemon's own clocks reaping what nobody touched |
| Disputes | `dispute` | Disputes opened, resolved by a solver, or undone by the parties themselves |
| Payment failures | `payment-failure` | Invoices the daemon cannot pay, how it recovers, and how many unresolved payouts it lets one payee pin |
| Anti-abuse bonds | `bond` | Bonds released, slashed and claimed |
| Adversarial | `adversarial` | Malformed, replayed, out-of-state and unauthorised messages |

This version implements the first three families in full. The other four are roadmap (section 14): their
tags are reserved, and no scenario carries them yet.

A scenario whose profile lacks a required capability is **skipped with a reason, never failed**, and a
skipped scenario is never counted as accepted.

### 8.2 Happy path

| Scenario | Narrative | Required evidence |
|---|---|---|
| `happy_sell` | Alice creates a sell order. Bob takes it and provides a payout invoice. Alice pays the hold invoice. Bob reports fiat sent. Alice releases. | Matching order ID on both screens and in relay events. Hold invoice paid by Alice and settled once. Payout received by Bob once. Both clients show completion. |
| `happy_buy` | Bob creates a buy order. Alice takes it as seller and pays the hold invoice. Bob provides a payout invoice and reports fiat sent. Alice releases. | Same evidence with maker and taker roles reversed. |
| `range_order` | Alice creates a sell order for a fiat range (one to three times the configured fiat amount). Bob takes it at twice the amount. The trade completes as `happy_sell`. | The book carries both bounds of the range. The daemon prices the take; the hold and payout settle once each. After settlement the daemon publishes the remainder as a new pending order of Alice's, for the fiat left of the range; her app lists it, and she cancels it with relay confirmation. |
| `range_buy_order` | Bob creates a buy order for the same fiat range. Alice takes it as seller at twice the amount and pays the hold; the trade completes as `happy_buy`. | Same evidence as `range_order` with the roles reversed: the remainder is a new pending order of Bob's, named by the trade key he sent with fiat sent, listed by his app and cancelled by him with relay confirmation. |
| `market_price_order` | Alice creates a sell order priced at market (no premium). Bob takes it. The trade completes as `happy_sell`. | The book carries the order with no sats amount. The daemon prices the take: the hold invoice asks for a positive amount within the per-order cap, the buyer invoice matches what the app requests, both settle once. |
| `full_trade_with_rating` | `happy_sell`, then Alice and Bob each rate the other with five stars. | After completion each app offers the rating; after each rating the app shows the trade rated and the daemon publishes a reputation event on the run relay. |

`happy_sell` and `happy_buy` are the two ordinary trades of the Mostro protocol. Mortsom expresses every
actor action as a real UI journey; nothing is sent on the protocol by the harness itself. Market pricing
needs the daemon's exchange-rate source to be reachable from the stack.

### 8.3 Cancellations

| Scenario | Narrative | Required evidence |
|---|---|---|
| `cancel_before_taken` | Alice creates a sell order and cancels it while pending. | Both apps and the relay show the order cancelled. No payment. |
| `cancel_after_take_unpaid` | Alice creates a sell order. Bob takes it and never provides an invoice. | After the daemon's taker window it publishes the order as pending again, cancelled or expired — a revision later than the take, never the original `pending`. Alice's app shows the same state. A republished order is closed by the cleanup sweep. |
| `taker_cancel_at_taken` | Alice creates a sell order. Bob takes it, provides his invoice and cancels before Alice pays. | The relay republishes the order as pending after the cancel; Alice's app shows it pending. The cleanup sweep closes it. |
| `cooperative_cancel` | Alice creates, Bob takes, Alice funds. Alice asks to cancel; Bob agrees. | Bob's app keeps the order active for the whole quiet window after Alice's request. After Bob's agreement both apps and the relay show it cancelled and Alice's hold payment fails (the escrow returns). |
| `unilateral_cancel_rejected` | As `cooperative_cancel` up to Alice's request; Bob does not agree during the window. | Bob's app keeps the order active and the relay publishes no `canceled` for the whole quiet window. Bob then agrees, which returns the escrow; that step is teardown, not the assertion. |

The maker's own screen is never proof of a cancellation: app v2 marks a funded order cancelled on the
request alone, while the daemon has only opened the cooperative flow and still holds the escrow.

### 8.4 Expirations

Every scenario here waits for a deadline the daemon owns, so each takes as long as the stack's limits
(section 5, `stack/limits.toml`) say. The proof is the daemon's published state after a baseline taken
before the wait, never a screen. App v2 drops a trade that never went active from its list once the
daemon closes it, so on the app the evidence is that the order is no longer held open: shown closed, or no
longer listed.

| Scenario | Narrative | Required evidence |
|---|---|---|
| `pending_order_expires` | Alice creates a sell order and nobody takes it. The test build asks the daemon for the lifetime the stack declares (`order_expiration_secs`, through `MORTSOM_ORDER_EXPIRY_SECS`), since the daemon's own default is hour-granular. | The relay shows the order `expired` after the baseline. Alice's app no longer holds it open. No payment. |
| `hold_invoice_expires` | Alice creates a sell order, Bob takes it and provides his invoice; Alice never pays the hold. | After the daemon's taker window the relay shows the order `canceled` (never `expired`, which is for orders nobody took). Neither app holds it open. No payment. |
| `buyer_invoice_deadline` | Bob creates a buy order, Alice takes it and pays the hold; Bob never provides his payout invoice. | After the window the relay shows the order `canceled`, neither app holds it open, and Alice's hold payment fails: the escrow came back. |

A fiat-payment window is not driven: the daemon has no such clock (`fiat_window_secs` stays undeclared in
`stack/limits.toml`), so an active order never expires on its own.

Rules for every scenario:

- Every wait has a predicate, a deadline and a failure category. No fixed sleeps.
- Visible states are mapped explicitly from `mostro-core` types. Loading, payout pending, escrow settled,
  payout completed, cancellation and expiration never collapse into one success state.
- A form-derived observation must carry the exact public order ID. A foreign form cannot authorize a payment.
- Negative assertions observe an explicit interval after the action, with a sequence cursor.
- Private app keys are never extracted to decrypt messages. Evidence comes from the UI, public relay events
  and wallet results.

New scenarios are added one family at a time, each scenario with its own row here and its own live
acceptance per platform. The earlier twenty-scenario catalogue remains a roadmap in
[docs/history/spec-v0.3.md](docs/history/spec-v0.3.md), section 10.

## 9. Identity and isolation

Each run starts with fresh app storage and new generated identities for Alice and Bob. The run observes both
public identities through the UI, proves they differ, relaunches each app and proves each identity persists.
Mnemonics and private keys are never exported.

Observers start before the actions they verify. A run refuses to create an order when the daemon identity
does not match, the relay is not ready or actor bootstrap failed. Only one run may use a given stack and
wallet set at a time; a second run reports the conflict before launching anything.

## 10. Value safety

These rules protect real funds, even on regtest, and are never relaxed for convenience:

- Reserve budget before any payment: the actual invoice amount plus the fee ceiling. Per-order and cumulative
  caps are mandatory; a missing cap is an error. Retries and resets never reset the cumulative budget.
- Write durable submission or payment intent before the irreversible UI or RPC action.
- Never blindly resubmit an order after a missing confirmation, and never retry a payment whose outcome is
  uncertain. An invoked payment without a terminal result stays in flight until accounting succeeds.
- Look up payments by hash in both directions: an invoice this node issued and an outgoing payment it made.
  A node that has never issued an invoice must not turn an outgoing payment into "unknown".
- Network clearance comes from the observed wallet and node networks. `localhost` does not prove regtest.
  Explicit mainnet confirmation remains fail-closed.
- Secrets use `env:` references, become `Secret` values immediately and never appear in arguments, build
  defines, logs or artifacts.

## 11. Cleanup and recovery

Cleanup is registered as soon as a submission can create an order, including uncertain submissions. New
run-owned order IDs are discovered from UI and relay observations so a missed confirmation does not orphan an
order.

On success, failure or interruption, in order:

1. Preserve sanitized UI, relay and wallet evidence.
2. Resolve or cancel every tracked order through the normal UI. `pay.cancel` cancels the order; it is not a
   navigation shortcut.
3. Verify terminal wallet outcomes; record outstanding hashes and escrow.
4. Close both sessions and stop owned processes, buses, displays and servers even when a step fails.
5. Remove profiles and credential storage only after every order and payment is resolved.
6. Report every cleanup failure. Unresolved cleanup blocks acceptance.

A sanitized recovery manifest is written as soon as an order may exist: run and stack identity, actor labels,
order IDs, invoice hashes, artifact provenance and references to protected storage. It contains no keys.
Unresolved runs retain actor profiles and credential data under the operator's private state directory with
owner-only permissions. Retention has no automatic expiry.

`mortsom cleanup <run-id>` reacquires the environment lock, validates the same stack and artifact, restarts
private runtime services on the retained storage, verifies the original public identities, and retries only
the required UI actions. Web rebinds the exact original loopback origin; if it cannot, recovery fails
explicitly. A missing profile, a changed identity or a changed stack is an unrecoverable-identity error, never
permission to create a fresh identity. Evidence is written before protected storage is removed; the durable
closed manifest is authoritative.

A killed run leaves active-runtime markers and later invocations fail closed. Markers are never removed just
to bypass that refusal.

## 12. Evidence and reports

Each run writes to ignored `artifacts/<run-id>/`: summary, timeline, per-actor screenshots and accessibility
trees, relay events, wallet payments, cleanup and environment records. Environment records include platform,
app revision and artifact digest, Flutter, Rust, browser and driver versions, stack fingerprint, daemon image
digest and version, declared limits and relay endpoints.

Evidence is sanitized: keys, mnemonics, credentials, NWC URIs and invoice contents never appear. Raw app and
service logs live only in actor-private directories and are removed on teardown. Linux screenshots capture
the actor's virtual display, never the operator's desktop.

Accepted results are recorded per platform in [docs/acceptance-ledger.md](docs/acceptance-ledger.md) with run
IDs and evidence locations. Skips and unsupported capabilities are visible and never count.

## 13. Validation and review

Implementation changes follow test-driven development. Before every commit:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Line coverage stays at or above 80% measured with `cargo llvm-cov` over the workspace; scenario narratives
count as tests. App contract changes run the app's Flutter tests and analysis and its documented Rust checks.
`mostro-core` stays pinned at `=0.14.5` with its Nostr dependency line until a separate reviewed commit
changes it.

Each completed change is reviewed independently before commit and the resulting commit is reviewed for
overlooked bugs. Commits follow Conventional Commits and never include secrets, private state or artifacts.

## 14. Deferred work

Documented as roadmap, not required by this version:

- The remaining scenario families of section 8.1: disputes, payment failures, anti-abuse bonds and
  adversarial messages.
- CPU and memory comparison of Linux and Web, and against the Android baseline, under matched conditions.
- Multi-scenario runs with in-process profile reset.
- NWC wallet mode for desktop clients.
- Any new Android emulator work.

## 15. Related documents

| Document | Purpose |
|---|---|
| [README.md](README.md) | Installation, quick start and command reference |
| [docs/HANDOFF.md](docs/HANDOFF.md) | Current state, open work and how to resume |
| [docs/acceptance-ledger.md](docs/acceptance-ledger.md) | Per-platform acceptance ledger |
| [docs/runbook.md](docs/runbook.md) | Failure categories and operator responses |
| [docs/history/](docs/history/) | Archived material: spec v0.3 with its plan and attempt ledger, the Android guide, the app v2 inventory and the crate rationale |
