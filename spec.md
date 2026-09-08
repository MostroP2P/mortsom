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

More scenarios follow once these four cells are accepted. A scenario, platform or benchmark outside this list
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
# Regtest environment, delegated to Ortsom.
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

Ortsom owns the regtest environment: `bitcoind`, the Nostr relay, `mostrod` and the LND nodes Alice, Bob and
Carol, with their persistent volumes and connection metadata. `mortsom stack up` runs `ortsom stack up` in the
configured sibling checkout; running Ortsom directly is equally valid.

`stack up` must:

1. Pull the configured Mostro image. A failed pull is an error, not permission to use a stale cached image.
2. Delegate creation or update to Ortsom with that image.
3. Verify that the running daemon container converges to the pulled digest.
4. Wait for relay, daemon and wallet readiness; validate regtest networks, daemon identity, advertised relay
   endpoints, channel liquidity and declared daemon limits.
5. Record the image digest, daemon version and a public stack fingerprint. Never record secrets.

Mortsom reads connection references and the daemon's public identity from the stack. It must not load the
solver secret to obtain a public key. Scenario teardown never removes persistent containers or volumes. A
stack reset is an explicit operator action; it is not evidence that an order was cancelled or a payment
refunded.

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
version. Its operating instructions are in [docs/legacy-android.md](docs/legacy-android.md).

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

| Scenario | Narrative | Required evidence |
|---|---|---|
| `happy_sell` | Alice creates a sell order. Bob takes it and provides a payout invoice. Alice pays the hold invoice. Bob reports fiat sent. Alice releases. | Matching order ID on both screens and in relay events. Hold invoice paid by Alice and settled once. Payout received by Bob once. Both clients show completion. |
| `happy_buy` | Bob creates a buy order. Alice takes it as seller and pays the hold invoice. Bob provides a payout invoice and reports fiat sent. Alice releases. | Same evidence with maker and taker roles reversed. |

Source narratives are Ortsom's `happy_sell` and `happy_buy`; Mortsom preserves their business assertions and
replaces protocol-native actor actions with real UI journeys.

Rules for every scenario:

- Every wait has a predicate, a deadline and a failure category. No fixed sleeps.
- Visible states are mapped explicitly from `mostro-core` types. Loading, payout pending, escrow settled,
  payout completed, cancellation and expiration never collapse into one success state.
- A form-derived observation must carry the exact public order ID. A foreign form cannot authorize a payment.
- Negative assertions observe an explicit interval after the action, with a sequence cursor.
- Private app keys are never extracted to decrypt messages. Evidence comes from the UI, public relay events
  and wallet results.

New scenarios are added one at a time after the four cells are accepted, each with its own row here and its
own live acceptance per platform. The earlier twenty-scenario catalogue remains a roadmap in
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

Accepted results are recorded per platform in [docs/ortsom-ui-parity.md](docs/ortsom-ui-parity.md) with run
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

- The remaining eighteen Ortsom scenarios: cancellations, expirations, disputes, ratings, range and market
  orders, invoice faults and payout caps.
- CPU and memory comparison of Linux and Web, and against the Android baseline, under matched conditions.
- Multi-scenario runs with in-process profile reset.
- NWC wallet mode for desktop clients.
- Any new Android emulator work.

## 15. Related documents

| Document | Purpose |
|---|---|
| [README.md](README.md) | Installation, quick start and command reference |
| [docs/HANDOFF.md](docs/HANDOFF.md) | Where execution stopped and how to resume |
| [docs/ortsom-ui-parity.md](docs/ortsom-ui-parity.md) | Per-platform acceptance ledger |
| [docs/runbook.md](docs/runbook.md) | Failure categories and operator responses |
| [docs/stack.md](docs/stack.md) | Workspace and crate rationale |
| [docs/mobile-v2-inventory.md](docs/mobile-v2-inventory.md) | App v2 screens and controls relevant to automation |
| [docs/legacy-android.md](docs/legacy-android.md) | Frozen Android emulator path |
| [docs/history/spec-v0.3.md](docs/history/spec-v0.3.md) | Previous specification, task plan and attempt ledger |
