# Mortsom technical specification

**Version:** 0.3

**Updated:** 2026-09-08

**Status:** active implementation plan; live validation in progress

**Target:** Mostro app v2 (`MostroP2P/app`, local `../mobilev2`)

**Default platform:** Linux

This document specifies the complete migration from Android emulators and the legacy app to lighter Linux and
Web clients. It is normative for new work. The previous specification (Git revision `ec22af1:spec.md`) is
historical; its Android implementation remains available, with no new Android work in this migration.
Everything in this repository is written in English.

A requirement below describes the intended result, not a claim that it is implemented. Section 18 records the
current evidence separately. Section 17 tracks 65 tasks: 26 completed, 5 in progress and 34 pending.
Live checkpoint acceptance remains Linux 0/2 and Web 0/2.

## 1. Objective and completion criteria

Mortsom must launch two independent copies of app v2, Alice and Bob, against a persistent local stack managed
by Ortsom. The operator chooses Linux or Web; Linux is the default. Android remains an explicit selectable
option.

### 1.1 Current review checkpoint (scope revised 2026-09-08)

The operator explicitly reduced the immediate target to **happy_sell and happy_buy on both Linux and Web**.
Implement and verify those four scenario/platform combinations, then stop for the operator to review the
work. Do not automatically continue with the other eighteen scenarios or resource benchmark repetitions.
The original twenty-scenario catalogue remains a deferred roadmap, not the current completion requirement.

This checkpoint is complete only when:

1. Stack setup/update, read-only doctor, client preparation and isolated Alice/Bob startup work through the
   documented CLI, using app v2 and the verified mostrod image.
2. happy_sell and happy_buy each pass through the real Linux UI and the real Web UI: four accepted cells,
   with the same business assertions and correlated order IDs, escrow and buyer payout evidence.
3. Every accepted run finishes with verified order/payment cleanup and releases its client resources.
   Unresolved failures retain protected recovery state and never count as passes.
4. Each invocation runs one scenario with fresh actors. Multi-scenario in-process profile reset is deferred;
   it must not silently reuse identities if that unsupported desktop invocation is attempted.
5. Relevant Rust/app checks and independent reviews pass. Update this spec and the evidence ledger with
   commands, source/artifact/daemon identities, actual results and remaining limitations.
6. Stop after the four cells pass and present the work for operator review. CPU comparison, additional
   journeys and further live benchmark repetitions require the next user instruction.

### 1.2 Deferred full migration

The remaining eighteen catalogue scenarios, multi-scenario reset, invoice faults, disputes and matched
resource benchmarks remain documented below as future work. Their phase gates do not block this smaller
checkpoint except where shared safety, lifecycle or truthful-evidence requirements are needed by its two
happy paths. Current execution order is Phase 0 → Phase 1 → Phase 2 → Phase 3, followed by the relevant
verification/documentation/review work from P7.5–P7.7. Phase 3 now includes happy_buy (task P4.2, moved without
renumbering for traceability). Do not begin the other Phase 4–7 scenario/benchmark tasks in this checkpoint.

Unit tests, a fake daemon, successful builds, onboarding, skipped cases and launch smoke tests do not count as
live scenario replications. A passing Ortsom invocation does not count as a passing Mortsom scenario.

## 2. Scope and ownership

| Component | Owner | Responsibility |
|---|---|---|
| Mortsom | This repository | Rust CLI, UI drivers, adapters, scenario narratives, safety, observers and reports |
| App v2 | `../mobilev2` | Flutter/Rust application, visible states, accessible controls and test bootstrap contract |
| Ortsom | `../ortsom` | Persistent regtest stack and reference scenario behavior |
| Mostro | `../mostro` | Daemon implementation and container image |
| Protocol | `../protocol` | Normative wire protocol |

Use sibling repositories to understand behavior; do not copy their source or import their scenario runner.
Mortsom owns an independent UI-driven runner. Protocol v1 gift-wrap transport is obsolete and must not be
accommodated. Reuse `mostro-core` protocol types and event constants. Keep its exact `=0.14.5` pin and
associated Nostr dependency line until a separate reviewed commit changes that pin.

The migration excludes new Android emulation work, legacy-app feature work, production deployment, arbitrary
protocol fuzzing and a mainnet benchmark. Existing mainnet protections remain mandatory even though acceptance
runs use real Lightning nodes on a local Bitcoin regtest network.

## 3. Architecture

```mermaid
flowchart TD
    CLI[Mortsom Rust coordinator] --> STACK[External Ortsom stack CLI]
    STACK --> ENV[Persistent regtest stack]
    CLI --> ADAPTER[App v2 domain adapter]
    ADAPTER --> LINUX[Linux AT-SPI driver]
    ADAPTER --> WEB[Web W3C WebDriver driver]
    LINUX --> LA[Alice isolated desktop]
    LINUX --> LB[Bob isolated desktop]
    WEB --> WA[Alice browser profile]
    WEB --> WB[Bob browser profile]
    LA --> ENV
    LB --> ENV
    WA --> ENV
    WB --> ENV
    CLI --> OBS[Read-only UI, relay and wallet observers]
    OBS --> REPORT[Sanitized evidence and resource report]
```

The coordinator depends on external boundaries for command execution, client sessions, Docker inspection,
relay transport, wallets, host configuration and clock. Tests inject fakes through those boundaries. A
platform-neutral `UiSession` exposes navigation primitives; `AppAdapter` turns domain actions into user
journeys. Scenario bodies never contain browser or AT-SPI details.

Keep the existing Rust workspace. Add desktop drivers without duplicating budgeting, observers, state
normalization, adapter logic or reporting. Existing Android-named shared interfaces may be reused initially;
names must not make Linux/Web depend on an Android SDK, AVD, Appium or adb installation.

## 4. Operator workflow and CLI

The intended public workflow is:

```sh
# Create/update and validate the persistent environment.
mortsom --ortsom-dir ../ortsom stack up
mortsom --ortsom-dir ../ortsom stack status

# Validate prerequisites without creating an order or paying anything.
mortsom doctor --platform linux
mortsom doctor --platform web

# Linux and app v2 are the defaults for Alice and Bob.
mortsom run happy_sell
mortsom run happy_sell --platform linux
mortsom run happy_sell --platform web
mortsom run happy_buy --platform linux
mortsom run happy_buy --platform web

# Optional existing build; validate it before scenario actions.
mortsom run happy_sell --platform linux --app-artifact /path/to/bundle/mostro
mortsom run happy_sell --platform web --app-artifact /path/to/build/web

# Reserved/frozen path; requires an explicit Android-compatible config.
mortsom --config android.local.toml run happy_sell --platform android

# Inspect unresolved work before explicitly stopping the environment.
mortsom cleanup <run-id>
mortsom --ortsom-dir ../ortsom stack down
```

These commands are the target contract; section 18 identifies pending pieces. `mortsom stack up` invokes
`ortsom stack up` as an external command in the selected sibling checkout. Running that Ortsom command
directly is equally valid. `run` attaches to an already ready stack and fails with actionable instructions if
it is absent; it does not implicitly create or destroy it.

One selection applies to both actors. Mixed Linux/Web pairs are outside this iteration. Unknown platforms fail
before building or starting clients. Android selection must neither silently use Linux nor claim unsupported
new capabilities. Doctor reports only the dependencies needed for its selected platform. Missing required
tools are reported before expensive builds.

## 5. Persistent environment and latest mostrod

Ortsom owns Bitcoin regtest, its LND topology, the Nostr relay, Mostro and the associated persistent volumes
and connection metadata. Mortsom consumes explicit connection references, daemon public identity and declared
limits. It must not load a solver secret merely to obtain a daemon public key.

On `stack up`:

1. Resolve the configured Ortsom checkout and executable.
2. Pull `mostrop2p/mostro:latest`; a failed pull is a reported failure, not
   permission to silently use an old cached image.
3. Delegate stack creation/update to Ortsom with that image selected.
4. Check the running daemon's image against the resolved digest. An existing
   container must converge to the requested image; an unchanged tag string
   alone does not prove that it was updated.
5. Wait for relay, daemon and wallet readiness and validate regtest networks,
   identity, advertised relay endpoints, liquidity and declared daemon limits.
6. Record the immutable image digest, daemon version and stack configuration
   fingerprint, excluding secrets.

Refresh `latest` at explicit stack updates, never in the middle of a scenario or benchmark. A run uses the
already verified immutable image. Report the actual running image, not the local checkout version or a
hardcoded label.

Scenario teardown must never remove persistent containers or volumes. Stack reset is an explicit regtest
recovery operation: preserve evidence, unwind orders/payments, invoke supported Ortsom lifecycle commands,
then revalidate. Never infer ownership from a container-name prefix alone or delete an unrelated operator
environment. Failed readiness must not trigger a destructive reset.

## 6. Configuration, networks and safety

The committed example is `config/desktop.example.toml`. Runtime configuration and `*.local.toml` remain
ignored. Secrets use `env:` references, become `Secret` immediately, and never appear in command arguments,
build defines, logs or artifacts. Daemon public identity and local relay URLs are non-secret.

Configuration declares the lane, platform prerequisites, app profile/source or artifact, stack checkout,
wallet references, relay endpoints, order amount, per-order cap, cumulative run cap and bounded deadlines. No
cap means error. Host paths and platform settings must be validated without requiring a source checkout when a
valid prebuilt artifact is supplied.

Local acceptance uses actual regtest LND payments. It is not a simulated-wallet claim and does not demonstrate
mainnet interoperability. Network clearance is based on observed wallet/node networks; `localhost` and lane
`local` do not prove regtest. Existing explicit mainnet confirmation remains fail-closed.

Reserve budget before a payment can begin. Include all orders in range and multi-order scenarios and account
for fees and pending payments according to the existing safety policy. Resetting an app, retrying
infrastructure or moving to the next scenario must never reset the cumulative run budget.

Only one active run may use a given exclusive stack/wallet set. Separate artifact directories must not defeat
environment locking. A second run reports the conflict before launching clients or making changes.

## 7. Client builds and public automation contract

Build app v2's Mortsom entrypoint, `lib/main_mortsom.dart`, with an explicit test-environment marker, daemon
public identity and local relay seeds. Disable public production fallback before subscriptions begin. Validate
discovered relay addresses from the client's network namespace; a Docker-internal hostname is not
automatically reachable by desktop or browser clients.

Follow the app's supported generation/build scripts. Regenerate Flutter Rust Bridge bindings through
`scripts/frb-generate.sh` before source builds to avoid Dart/Rust content-hash mismatches. Do not hand-edit
generated bindings. For Web, build the supported Rust/WASM bundle before the Flutter Web artifact. Retain
required shared-memory support and serve cross-origin isolation headers.

Record source revision, dirty-state indicator, build flags, tool versions and artifact digest. Both actors use
the same artifact. Git revision and dirty flags do not capture dirty source contents or detect every edit
made during a build; the bundle digest identifies the actual captured bytes. Stack fingerprint version
`sha256-mortsom-stack-v1` hashes `mortsom-stack-v1\0`, the JSON byte length as an unsigned 64-bit big-endian
integer, and the serialized public `StackSnapshot`. Struct field ordering belongs to this version; endpoint
maps have sorted keys. Change the algorithm version if serialization changes. JSON files are individually
atomic, not one multi-file transaction. Directory synchronization assumes a stable local directory hierarchy;
concurrent ancestor-path replacement is outside these helpers' guarantees. A prebuilt artifact must pass visible environment/identity checks; path existence alone is
insufficient.

The app owns `docs/automation-contract.md`. Stable IDs cover onboarding, navigation, node/relay settings, form
fields, order identity/status, invoices, cancellation, disputes, ratings and errors. Add missing controls as
ordinary accessible product UI, with coordinated app/harness review and widget tests. No test-only
business-action endpoints are allowed.

### 7.1 Linux

Each actor owns a native app process, virtual X display, private session bus, AT-SPI accessibility bus, Secret
Service instance and separate XDG storage. XDG isolation alone is insufficient because shared Secret Service
can reuse another actor's identity. Allocate displays without a fixed-port race.

Readiness waits must prove accessibility registration and an unlocked usable credential collection before app
launch. Use bounded predicates and child liveness checks. Disable incidental desktop service activation that
can leave portal or filesystem mounts outside the owned process lifecycle.

Flutter's Linux engine may not export `Semantics.identifier`. In Mortsom test builds the app exposes
`[mortsom:<identifier>]<readout>` as its accessible name. Keep this namespacing out of normal production
labels. Use public AT-SPI interfaces for action, focus, text entry, scrolling and state observation. A
transport success must not substitute for visible input acceptance.

### 7.2 Web

Serve the release bundle on loopback with `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`. Limit requests to files inside the canonical bundle root,
including symlink/traversal checks. Own every request task and terminate it when the server shuts down.

Use W3C WebDriver and separate browser profiles for Alice and Bob. Keep the browser sandbox enabled. Match
exact `flt-semantics-identifier` values with properly escaped selectors; IDs can include relay URLs and
dynamic order IDs. Use public DOM/accessibility interactions only. No JS calls into Dart, Rust, application
stores or private state are permitted.

Flutter may retain offscreen semantic fields and rebuild them after focus or scroll. Re-find elements after
transitions; scroll through normal user actions and wait for interactability. Do not add fixed sleeps to mask
animation races. Retain isolation headers for WASM workers and record browser/driver versions.

## 8. Identity and scenario isolation

Every new scenario starts with fresh Alice/Bob app storage and distinct generated test identities. Observe
public identities through the normal UI, prove they differ, relaunch each app, and prove each identity
persists. Do not export mnemonics to reproduce a run. Evidence uses opaque actor labels where possible.

Within a persistence scenario, preserve that actor's profile across the intended restart. Between scenarios,
close sessions and reset profile, secure storage, navigation and order subscriptions. Wallets may remain part
of the persistent stack, but observations use scenario-specific invoice hashes and order IDs. Never use a
global balance delta as proof of a particular payment.

Start observers before relevant actions. Reject mismatched daemon identity, unready relay or failed actor
bootstrap before creating an order. A bootstrap check in a report must influence the run outcome rather than
being informational.

## 9. Scenario actions and assertions

Scenarios express business actions through the shared app adapter. Linux and Web run the same narrative. Reuse
`mostro-core` wire types and maintain an explicit mapping to visible app states. Loading, payout pending,
escrow settled, payout completed, cancellation and expiration must not collapse into a misleading success
state.

The supported manual external-wallet path is valid: read the invoice presented by the app, pay it on the
actor's dedicated LND wallet, and type the buyer's wallet-generated invoice into the normal form. Wallet
actions perform the external user's wallet role; they must not publish create/take/release/cancel protocol
messages. NWC remains a separate capability with its own UI setup.

Every wait has a predicate, deadline and failure category. Negative assertions observe an explicit interval
starting after the action, with a sequence cursor so earlier events cannot satisfy them. Expiration waits
derive from declared daemon limits plus bounded sweep/propagation allowance. No fixed scenario sleeps.

An accepted trade joins the visible order ID, daemon-signed public relay observations, hold invoice hash,
buyer payout hash, amounts and terminal wallet outcomes. A completed-looking screen alone is insufficient. Do
not obtain private app keys to decrypt messages just to reproduce Ortsom's assertions; use the UI plus
legitimately observable public events and wallet results.

## 10. Scenario catalogue: current two and deferred eighteen

Only rows 1–2 are in the current checkpoint. Rows 3–20 are deferred until the operator reviews those four
Linux/Web results and requests further work. Existing non-value diagnostics may finish safely, but they do
not expand the checkpoint or count toward it.

The source catalogue is `../ortsom/docs/scenarios.md`; reference bodies live under
`../ortsom/crates/ortsom/src/scenarios/`. Preserve their business assertions while replacing protocol-native
actor actions with real UI journeys. Register stable source names and, in the deferred milestone, the `ortsom-core-20` tag; existing
aliases may remain but cannot count as additional scenarios.

| # | Source scenario | Required live assertions and prerequisites |
|---|---|---|
| 1 | `happy_sell` | Alice sells, Bob takes/provides invoice/reports fiat, Alice releases; both show completion and matching escrow/payout settle exactly once. |
| 2 | `happy_buy` | Bob creates a buy, Alice takes as seller; same correlated settlement with reversed maker/taker roles. |
| 3 | `cancel_before_taken` | Maker cancels pending order; book removal and legal terminal UI, with no payment. |
| 4 | `cooperative_cancel` | Funded active order requires both actors' agreement; cancellation is visible to both and seller escrow is refunded. |
| 5 | `unilateral_cancel_rejected` | First cancellation opens cooperation only; no premature cancellation/refund during the observation window; complete cooperative cleanup afterward. |
| 6 | `cancel_after_take_unpaid` | Taker abandons without completing payment prerequisites; a new post-take event returns the order to a legal pending/terminal state after the declared deadline. |
| 7 | `pending_order_expires` | Set expiry through the create-order UI; untouched order expires and leaves the book. Requires a visible expiry control and actual daemon-enforced deadline. |
| 8 | `hold_invoice_expires` | Buyer supplies a valid invoice, seller leaves hold unpaid; observe the daemon's new pending/canceled/expired transition and no settlement. |
| 9 | `buyer_invoice_deadline` | Buyer never submits payout invoice; observe the declared deadline and new legal recovery/terminal state, without matching the initial pending event. |
| 10 | `full_trade_with_rating` | Complete a trade; both rate through UI; rating acceptance and resulting reputation/history are observable without duplicate settlement. |
| 11 | `range_order` | Create a range, take an intermediate fiat amount, verify priced amount and remainder identity/amount, complete taken portion and clean remainder. |
| 12 | `market_price_order` | Select market pricing through UI, observe resolved sat amount, enforce caps against it and verify payout with declared fee tolerance. |
| 13 | `dispute_by_buyer` | Buyer reports fiat and disputes; both show dispute and a matching public dispute event exists; prove a supported cleanup path before funding. |
| 14 | `dispute_by_seller` | Seller disputes an active trade; both UI states and public dispute identity correlate; prove supported cleanup before funding. |
| 15 | `dispute_then_cooperative_cancel` | Open an untaken dispute, cooperatively cancel through both clients, observe closed dispute and refunded seller escrow. |
| 16 | `invalid_buyer_invoice` | Submit a malformed/expired/wrong-amount invoice through UI; rejection must retain a recoverable invoice-required state and cause no illegal payment. Distinguish local validation from daemon rejection. |
| 17 | `payout_invoice_below_the_expiry_floor` | Submit an LND invoice below the declared minimum expiry; rejection and no advancement. Requires configurable invoice expiry and known daemon floor. |
| 18 | `payment_fails_on_release` | Invalidate the accepted buyer invoice on the controlled wallet, release through UI, observe payout failure/retry or replacement request, then recover payment and account for escrow. |
| 19 | `buyer_invoice_expires_mid_flight` | Make the previously accepted invoice unpayable after activation, release, submit a fresh invoice through UI, and prove recovery without duplicate payout. |
| 20 | `payout_slots_capped_per_destination` | Release trades to controlled buyer hold invoices up to the declared destination cap; the next receives no HTLC until a slot frees, then dispatches automatically; settle and account for every trade. |

This prioritizes ordinary user journeys, deadlines, dispute visibility and payment recovery.
`pending_order_expires` replaces the earlier provisional fairness candidate because it covers a common
user-visible deadline at lower resource cost. Destination-cap behavior remains in the required set.

`gated_payouts_do_not_starve_other_destinations` is an additional follow-up: fill Bob's cap and all declared
dispatch pressure, then prove an ordinary payout to Alice proceeds. It requires extra simultaneous trades and
budget; it is not a substitute for one of the twenty unless the matrix is explicitly revised with a
source-backed reason and independent review.

Solver resolutions, anti-abuse bonds, malformed encryption, unauthorized protocol actors and packet replay
remain outside this initial matrix. Do not fabricate UI parity for actions a normal client cannot express.

### 10.1 Missing app capabilities

Missing expiry/range/rating controls or accurate states are implementation work, not waived assertions. Add or
repair them in app v2 with accessible identifiers, visible user feedback, contract documentation and tests.

Disputes require special preflight: Mortsom must prove a user-accessible exit and refund path for the tested
untaken dispute. It cannot use an imported solver key or protocol-native admin action as a hidden cleanup
shortcut. If a dispute is taken or the supported UI cannot unwind it, stop and report unresolved escrow; do
not mark the scenario accepted or launch more funded cases. Fix the product workflow or explicitly revise the
plan before claiming completion. Destroying regtest volumes is not evidence of a refund.

Payment fault injection is confined to invoices and HTLCs on run-owned regtest wallets. It never edits
app/daemon databases or changes orders. Each fault declares its prerequisite, injection point, expected
visible response, negative payment assertion and recovery/cleanup route.

## 11. Error handling, cleanup and recovery

Use typed failure categories: product, automation contract, infrastructure, relay, wallet and environment
misconfiguration. Do not infer a category from message text. A missing control is not evidence that an order
is closed. A successful input API call is not evidence that the app accepted the value.

Register cleanup as soon as submission can create an order, including uncertain submission outcomes. Discover
new run-owned order IDs through public UI and relay observations to avoid orphaning an order when confirmation
is missed. Never blindly repeat submit/release/pay after a timeout.

On success, failure or interruption:

1. Preserve sanitized UI/relay/wallet evidence before closing clients.
2. Resolve or cancel every tracked order through supported UI, including range
   remainders and all destination-cap trades.
3. Verify terminal wallet outcomes; record outstanding hashes and escrow.
4. Close both client sessions, request tasks and owned process groups even if
   one cleanup step fails; retain enough handles to retry failed cleanup.
5. Remove owned temporary profiles and credential storage only after all
   orders/payments are resolved. For unresolved orders retain protected actor
   storage as described below; still stop owned processes, buses and displays.
6. Report every cleanup failure. Unresolved cleanup prevents acceptance.

### 11.1 Durable UI recovery

Persist a sanitized recovery manifest as soon as an order may exist. It records run/stack identity, actor
labels, order IDs, invoice hashes, artifact provenance, cleanup steps and references to protected storage; it
contains no private keys. Store recoverable actor profiles and Linux credential-service data outside the
repository under the operator's private state directory, with owner-only permissions. Never export mnemonics
or private app keys to the manifest.

If UI cleanup fails with unresolved orders, stop processes but retain those profiles and their Secret Service
state. A subsequent `cleanup <run-id>` reacquires the environment lock, validates the same stack and app
provenance, starts new private runtime services using retained storage, verifies the original public actor
identities and retries only the required normal UI actions. Web reuses the protected browser profile and its
original loopback origin (scheme, host and port);
record and rebind that origin because Web storage is origin-scoped. If the original port cannot be rebound,
report an explicit recovery failure instead of silently using a new origin. Linux reuses both app and
credential data. A missing profile or identity mismatch is an explicit
unrecoverable-identity error, never permission to create a fresh identity and call cleanup successful.

Track retained storage and recovery status in `cleanup.json`. Retention lasts until confirmed order/payment
resolution and an explicit successful cleanup, not an automatic expiry that destroys the only recovery
identity. Cleanup must never remove unresolved storage through a generic temporary-directory sweep. A stack
reset may be needed for regtest diagnostics, but record unresolved outcomes before reset; disappearance after
reset is not proof of cancellation.

A subsequent clean run must not inherit identities or browser profiles. Refuse a funded run on the same
wallet/stack set while tracked orders or held payouts remain unresolved; recover them first.
Infrastructure retries are bounded and explicitly configured; they preserve the failed attempt and cumulative
budget. Product, contract or payment-correlation failures must not be retried into a hidden pass. For
development, fix the cause and start a new recorded run.

## 12. Reports and reproducibility

Keep existing summary, timeline, UI screenshots/hierarchy, relay events, wallet payments and cleanup artifacts
under ignored `artifacts/<run-id>/`. Extend reports with platform, app revision/artifact digest, dirty state,
Flutter/Rust/browser/driver versions, stack identity, actual daemon image version/digest, declared limits,
seed relay endpoints and lane. Resource sampling data is deferred with section 14.

Record scenario source name, actor roles, attempt number, timings, required capabilities, status, explicit
failure category and cleanup status. A skip or unsupported capability is visible and never counts as an
accepted checkpoint cell. Persist a per-platform acceptance ledger with run IDs and sanitized evidence locations.

Sanitize process errors as well as successful output. Raw app/service logs may exist only in actor-private
directories with restrictive permissions and must be removed on teardown; do not publish them. Never capture
the operator's real desktop when taking a Linux screenshot. Keys, mnemonics, credentials, NWC URIs and
sensitive invoice content must not leak into evidence.

## 13. Automated validation and review

Follow TDD for implementation changes. Add meaningful tests for configuration, platform selection, command
plans, redaction, actor isolation, bounded waits, state mapping, budgeting and cleanup failures. Exercise
desktop coordinator paths through injected sessions, including partial startup and persistent-stack
non-destruction. Test profile cleanup when executables are missing and static server shutdown with an
in-progress request.

Before every commit:

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Maintain at least 80% line coverage with `cargo llvm-cov`; scenario narratives are tests rather than
production code under test. App contract changes require relevant Flutter tests/analyze and the app's
documented Rust checks. New or updated dependencies require a latest-stable check, respecting the explicit
Mostro/Nostr pin exception.

Use independent review subagents as requested: review each completed change before commit, then review the
resulting commit for overlooked bugs. Resolve findings and rerun affected checks/live scenarios. Use
Conventional Commits, separate completed specification tasks, and never commit secrets or artifacts.

## 14. CPU and memory comparison (deferred after operator review)

Measure Linux and Web after correctness, using the same physical host, daemon image digest, app revision,
relay, wallet state, scenario order, amounts, headless policy and declared deadlines. Run sequentially so
modes do not compete. Exclude Flutter/Rust compilation from execution measurements and report cold
build/startup costs separately.

Collect at a documented sampling interval (initial target: one second):

- Cumulative process-tree CPU seconds and mean CPU cores used, with a stated
  percentage convention; distinguish lifecycle/startup, idle and scenario work.
- Combined RSS and peak sampled RSS for both clients plus required helpers:
  Xvfb, private buses/AT-SPI/keyring for Linux; browser subprocesses,
  ChromeDriver and Web server for Web. RSS can double-count shared pages;
  include PSS when available and disclose the limitation.
- Shared stack/container CPU and memory separately, preventing shared daemon
  costs from being attributed to one client platform.
- Startup time, scenario duration, total elapsed time, pass/failure counts and
  number of open processes after teardown.

Use at least three complete measured repetitions per platform, alternating platform order. Report median and
range, raw sanitized samples, measurement boundaries and host/tool versions. Idle comparison uses a declared
observation window after both clients are ready; it does not introduce sleeps in scenarios. Do not claim CPU
savings from RSS alone or present failed/incomplete suites as comparable performance results. Do not start
Android emulators for this study; use an existing comparable Android baseline only if one already exists and
label its limitations. Otherwise report Linux versus Web only.

## 15. Dependencies and implementation risks

| Risk | Planned resolution |
|---|---|
| Linux semantics IDs absent | Test-build name prefix plus app-owned contract and widget coverage |
| Linux text API reports success without editing | Focus/edit through public accessibility; verify value and form response before submit |
| Web semantics offscreen or rebuilt | User scrolling/focus, exact selectors, fresh lookup and bounded interactability waits |
| Shared credentials or leftover services | Private DBus/Secret Service and profiles; readiness and failure-cleanup tests |
| Dart/Rust binding mismatch | Official regeneration before build; record artifact provenance |
| Docker relay address unreachable from host | Verify advertised endpoint reachability and test relay restrictions |
| App conflates pending payout with completion | Repair user-visible state mapping and assert actual wallet settlement |
| Missing dispute exit | Prove UI cleanup before funding; repair app workflow and revalidate |
| Expiry or payout limits differ from assumptions | Read declared stack limits and verify behavior against running daemon |
| Cap test consumes too many resources | Use explicit small test limits supported by Ortsom, budget every trade, preserve assertions |
| Multiple-run interference | Lock the environment/wallet set, scope events, reset clients between scenarios |

## 16. Delivery artifacts

The delivered change includes the Rust implementation, coordinated app v2 contract fixes, CLI/config examples,
operator setup and recovery documentation, the current four-cell scenario acceptance ledger, reviewed commits
and required automated check results. The standalone resource comparison belongs to the deferred milestone.

Document prerequisites instead of relying on tools manually installed during development. Include headless
Linux and Chromium/WebDriver setup, supported paths and failure messages. A user must be able to reproduce
acceptance after recreating the stack and building the app from documented source revisions.

## 17. Sequential implementation plan and exit gates

For the current checkpoint execute Phases 0–3 in order, with happy_buy moved into Phase 3. Close preceding
active gates before later active phases. Tasks explicitly deferred by section 1.2 are not current gates. Existing
exploratory builds and live probes are recorded as completed narrow tasks where supported by evidence;
these diagnostic results do not imply that any intervening phase has passed its gate.

### 17.1 Task status convention

- `[x] DONE`: the narrowly described task is completed with the evidence
  stated. This does not imply the containing phase has passed its exit gate.
- `[ ] IN PROGRESS`: code or a partial result exists, but required validation
  or corrections remain. Do not count it as completed.
- `[ ] TODO`: implementation or acceptance work remains.

Gate status is separate from task status. Diagnostic live attempts are allowed to expose missing requirements;
they cannot waive an earlier incomplete gate. When updating a checkbox, add a commit, check result or live run
ID. Do not mark a task done solely because its function or file exists.

### 17.2 Phase 0 — Specification and baseline

**Status:** completed. **Primary files:** `spec.md`, `docs/ortsom-ui-parity.md`, `docs/stack.md`.

- [x] **P0.1 DONE — Inventory the existing design and sibling contracts.** Read Mortsom's previous spec,
  Ortsom's catalogue and source families, and app v2's build/accessibility contract. Ownership is specified in
  sections 2–3.
- [x] **P0.2 DONE — Establish the source catalogue and initial matrix.** Identify twenty distinct source
  narratives; section 10 records the selected scope and the reason for changing the provisional fairness
  candidate.
- [x] **P0.3 DONE — Establish the existing Rust baseline.** Earlier workspace library/integration tests
  passed. Fix the current clippy large-error warning with boxed device-startup errors in reviewed commit
  `b4af42d`.
- [x] **P0.4 DONE — Record the migration ownership amendment.** Reviewed commit `ec22af1` established
  persistent-stack and Linux/Web execution gates; this version expands and supersedes that short amendment.
- [x] **P0.5 DONE — Review and commit the complete specification.** Independent review checked source
  fidelity, phase counts, actual bootstrap evidence and zero trade passes. Corrected invoice ordering, event
  cursor timing, secret-free stack metadata and durable UI recovery including Web origin. The twenty-row
  ledger and local links were checked; fmt, clippy and workspace tests passed before this documentation
  commit.

**Gate G0: passed.** Specification review findings are resolved; phase tasks, the twenty-case matrix and
current evidence agree.

### 17.3 Phase 1 — Stack integration, configuration and CLI

**Status:** completed. **Primary files:** coordinator `src/stack/`, `src/cli.rs`, `src/config/`,
`src/desktop.rs`, `src/doctor/`, `config/desktop.example.toml` and CLI integration tests.

- [x] **P1.1 DONE — Start the real reference environment.** The external Ortsom stack command successfully
  brought up the development regtest topology.
- [x] **P1.2 DONE — Update the development daemon image.** Pulled latest and recreated the running daemon on
  the digest recorded in section 18, retaining the stack identity. This is a one-time verified update, not
  proof that the new Mortsom update command already converges existing containers correctly.
- [x] **P1.3 DONE — Complete the stack command adapter.** Lifecycle commands delegate with a pinned
  Compose project and validate digest, ownership and readiness. Live stack setup passed; all six container
  identities stayed unchanged across failed run `20260908T141446Z-8f0617eeb9de`. Explicit subsequent regtest
  down/up succeeded and revalidated the same immutable image. Durable public metadata is covered by P1.8.
- [x] **P1.9 DONE — Consume a secret-free stack descriptor.** Reviewed Ortsom commits `8e82712`, `029e90e`,
  `1552992` and `566a53e` export versioned status, declared limits, inspected endpoints and atomic container
  image identity. Mortsom validates those fields without opening combined solver TOML. Required records,
  nonzero limits, image/digest consistency, local endpoints and credential paths have regression coverage;
  149 coordinator tests, CLI tests and full clippy passed, with independent review. Live read-only status
  confirmed the pinned image. Persistence of these facts belongs to P1.8.
- [x] **P1.4 DONE — Complete platform/configuration selection.** Linux is the default; Web/Android and
  explicit configuration selection have CLI regressions. Desktop uses shared `[ui]` deadlines without AVD
  placeholders; Android-only commands require `[android]`. Cleanup follows the platform recorded before
  client startup, rejects conflicting metadata and explicitly reports unsupported desktop recovery. Artifact
  commands load only offline settings; publication preserves platform-specific known-secret resolution.
  Independent review passed after compatibility fixes; 167 coordinator tests, seven CLI integration tests,
  formatting and full workspace strict clippy passed on 2026-09-08. Desktop recovery remains P2 work.
- [x] **P1.5 DONE — Finish artifact preparation.** Supported source generation/build scripts and
  source-absent prebuilt validation work. Real Linux and Web source builds, private capture, verification
  and capture removal passed, reporting app 2.0.0. Source revision/dirty flags, selected tools and executed
  arguments are recorded; prebuilt history remains explicitly unknown. Regression checks cover source
  context, changed revision/version, malformed tool output and secret redaction. See P1.8 for persistence.
- [x] **P1.6 DONE — Implement platform-aware doctor.** Source-build diagnostics passed Linux 38/38
  and Web 35/35; prebuilt paths passed Linux 21/21 and Web 17/17. Selected host tools, executable app
  scripts, codegen pin, native development libraries and Web nightly/rust-src are checked before any run
  build. Explicit relative/absolute Xvfb paths are honored. A real CLI invocation with Flutter/Xvfb absent
  exited 2 before generation, client startup or orders. Network checks separately verify signed daemon
  identity/protocol, endpoints, regtest and liquidity before clients start. Doctor uses only daemon reads,
  NWC get_info/get_balance and LND GetInfo/ChannelBalance; LND mutation permissions are not proven by the
  reported backend-supported methods. Independent review corrected the relative-path regression;
  coordinator/CLI tests and strict clippy are recorded with this task's commit.
- [x] **P1.7 DONE — Lock stack/wallet ownership independently of artifacts.** The canonical-HOME
  Mortsom CLI lock precedes configuration loading and all stack/client lifecycle mutation dispatch, and
  stays held until the command completes. It serializes run, cleanup, app/device/relay commands and stack
  up/down independently of artifact or checkout paths. Four lock unit tests plus the cross-process CLI
  contention test passed with independent review; home aliases and release are covered. Direct Ortsom,
  another HOME and other users are outside this guard. Durable unresolved-work exclusion remains P2 work.
- [x] **P1.8 DONE — Persist immutable environment metadata.** Before client startup and in final reports,
  persist the captured bundle digest, source/build provenance, public stack fingerprint and daemon version
  observed by preflight. Real failed run `20260908T141446Z-8f0617eeb9de` retained those facts. Individual JSON
  files use private staging, file sync, atomic replacement and directory sync; newly created directory
  ancestors are synced, including on retries. Symlink-target and directory durability regressions passed.
  Independent review found no remaining blocker; 179 coordinator and seven CLI tests passed. Captured
  bundle and Web server ownership tests and real smoke checks passed on both platforms. These facts do not
  provide protected actor recovery, which remains P2.12.

**Gate G1: passed.** CLI/config/doctor and update/readiness checks passed; real `stack up` converged to the
requested digest, and the failed run demonstrably retained all persistent stack containers. This gate does
not claim successful order cleanup or any accepted trade.

### 17.4 Phase 2 — Builds and isolated Linux/Web clients

**Status:** in progress. **Primary files:** `crates/desktop-driver/src/`, coordinator `src/run/farm.rs`,
`src/bootstrap.rs`, `src/run/mod.rs`, app v2 `lib/core/automation/`, Mortsom entrypoint and automation
contract tests.

- [x] **P2.1 DONE — Prove app v2 builds locally on both targets.** Native release and Web release/WASM builds
  succeeded after official binding regeneration.
- [x] **P2.2 DONE — Prove Linux accessibility discovery.** Isolated native smoke observed the environment
  marker and walkthrough controls through AT-SPI.
- [x] **P2.3 DONE — Prove Web accessibility discovery.** Isolated Chromium smoke observed the same environment
  marker through the public semantics DOM.
- [x] **P2.4 DONE — Prove actual onboarding and restart identities.** Linux run
  `20260908T100217Z-a9eaf0273a29` and Web run `20260908T100434Z-7f107aaadcd8` passed both actors'
  bootstrap/identity checks. Their trade assertions failed and are explicitly not part of this completion.
- [x] **P2.5 DONE — Finish native interaction correctness.** Existing `native.rs` and `linux.rs` expose
  AT-SPI actions/text/state. Add focus and scroll handling, verify actual editable values and visible form
  feedback, and handle transient stale nodes without hiding permanent transport errors. Reproduce the
  empty-field failure before fixing it.
- [x] **P2.6 DONE — Verify Web text entry and readback.** Public DOM scrolling and W3C editing now
  re-find fields after scroll, focus and clear, retry only named transient element errors within a deadline,
  and verify the actual value property. Regressions reproduce ignored edits, stale/non-interactable nodes,
  permanent errors and input redaction. The real Web form accepted fiat 10, method Cash and fixed sats 100;
  no order was submitted and client/server/captured-bundle cleanup passed. Fifteen desktop tests, formatting
  and strict workspace clippy passed with independent review. Submit uncertainty remains P3.2; lifecycle
  remains P2.8. The same no-submit form probe also passed on Linux.
- [x] **P2.7 DONE — Verify bounded Linux lifecycle ownership.** Private services gate app launch on
  readiness/liveness. Failed startup and app/service shutdown preserve handles and files for retry; service
  handles remain owned across cancellation. Linux process cleanup keeps its leader unreaped until the owned
  group has no running members, avoiding PID reuse and premature profile deletion. Real leader-exit,
  sibling-isolation, startup-failure and cancelled-shutdown regressions pass. The real copied-bundle Linux
  form probe passed again with complete client cleanup. Reviewed commit `eb25f03` covers the process group,
  not descendants that deliberately detach into another group. Durable actor storage remains P2.12.
- [x] **P2.8 DONE — Verify bounded Web lifecycle ownership.** Runtime/profile ownership precedes startup
  awaits. ChromeDriver binds its own ephemeral port; readiness checks its liveness and owned startup output.
  Failed session/navigation and driver-kill retries retain ownership until stop is verified. Server shutdown
  retains its join handle across cancellation and drains request tasks. Twenty-three desktop tests and a real
  Web form/cleanup probe passed. Reviewed commit `eefafaa`; exhausted shutdown remains a failure, not a
  claim of cleanup or restart recovery.
- [x] **P2.9 DONE — Inject desktop session construction.** `RunDeps` supplies `DesktopFactory` and owned
  `DesktopServer` boundaries. Four coordinator regressions prove independent actor descriptors, second-actor
  construction failure cleanup, same-session shutdown retry and sibling/server cleanup after exhausted
  retries. The runner retains the same session through its bounded second shutdown attempt; exhausted
  failures are reported before remaining in-memory handles are dropped. Restart recovery remains P2.12.
  Independent review passed; the complete pre-join-fix workspace run passed 531 tests, followed by targeted
  desktop/farm reruns, formatting and strict workspace clippy after the join fix.
- [x] **P2.10 DONE — Enforce fresh actors for the checkpoint; defer in-process reset.** The CLI resolves
  names/tags and rejects desktop selections containing more than one distinct scenario before loading
  stack/configuration or building clients; the coordinator repeats this guard before creating artifacts.
  Separate commands allocate fresh actor storage. Offline CLI regressions cover Linux/Web named batches
  and tag expansion; unit checks preserve duplicate-name deduplication and existing Android batching.
  Independent review passed. Full in-process profile/subscription reset remains deferred.
- [x] **P2.11 DONE — Finish app contract coordination.** Reviewed app v2 commit `e83cd8f` adds test-only
  native semantic prefixes (including direct nodes), fixes desktop sidebar navigation, exposes the manual
  invoice's requested amount/order reference, and keeps escrow-settled payouts nonterminal until Success.
  Production labels and test-relay restrictions are preserved. Full checks passed: 338 Flutter tests,
  412 Rust tests, four armed Linux accessibility tests, Flutter analysis and strict Rust clippy.
  Two unarmed platform tests and eighteen pre-existing Rust tests are intentionally skipped/ignored.
  The shared automation contract documents these coordinated changes; live trade acceptance remains G3.
- [x] **P2.12 DONE — Implement protected recovery storage.** Add durable manifest and actor-storage ownership
  records before a UI submission can create an order. Separate process shutdown from profile deletion; test a
  coordinator restart followed by UI-only cleanup with the original identities, missing storage, mismatched
  stack and successful final deletion.

**Gate G2:** both platforms pass fresh two-actor bootstrap and restart plus input/navigation smoke, injected
partial-failure tests and verified complete client cleanup. **Gate accepted on 2026-09-08:** independent
review matched all gate requirements to the real Linux/Web probes and injected failure tests; final workspace
checks passed (604 tests, no failures, one existing ignored integration test), formatting and strict clippy.
Automatic orphan reconciliation after SIGKILL remains unsupported and fails closed, as documented below.

### 17.5 Phase 3 — Two fully correlated happy paths and review checkpoint

**Status:** diagnostic implementation in progress; zero live passes. **Primary files:**
`scenarios/src/smoke/happy_sell.rs`, `scenarios/src/flow.rs`, v2 adapter `src/screens/{nav,orders}.rs`,
`src/states.rs`, scenario-kit `actions.rs`, `evidence.rs`, `waits.rs`, `cleanup.rs` and wallet/relay
observers.

- [ ] **IN PROGRESS — P3.1 Repair and verify creation.** Read back fiat, payment method, pricing mode and sats
  through public interfaces before submit; assert enabled state. Wait for a new order ID and public pending
  event. Preserve the failed Linux/Web form cases as meaningful regressions.
- [ ] **IN PROGRESS — P3.2 Handle uncertain submission safely.** Capture known order IDs and observer cursor before
  submit; register provisional cleanup, resolve any new run-owned order after timeout, and never blindly
  republish.
- [ ] **TODO — P3.3 Validate discover/take and invoice steps live.** Match the exact order ID in Bob's book
  and take it. For a sell order, Bob first generates and submits the buyer payout invoice through normal UI;
  only then does Alice read and pay the seller hold invoice. Reserve spend before initiating payment and track
  both hashes.
- [ ] **IN PROGRESS — P3.4 Validate state and final settlement.** App review corrected premature completion:
  PurchaseCompleted now maps to Success, escrow settlement remains payout-pending, and premature rating
  navigation is blocked. Provider/widget regressions and harness normalization pass. Drive fiat-sent and release UI, distinguish
  intermediate payout states from completion, verify both clients' terminal state plus matching wallet
  amounts/outcomes and absence of duplicate settlement. Correct inaccurate product state mapping explicitly.
- [ ] **IN PROGRESS — P3.5 Make cancellation/teardown truthful.** Fixed missing cancel control returning
  AlreadyClosed for active, loading or unknown states; the regression failed before the fix and all 30 v2
  adapter tests passed afterward, with independent review. Cancelled currently acknowledges the confirmation
  tap; coordinator cleanup still verifies terminal state. App v2's escrow-settled mapping is now nonterminal; the coordinator waits for final payout rather than
  treating that state as AlreadyClosed.
  Verify a positive terminal state plus payment outcome or a completed cooperative cancellation and refund;
  surface unresolved cleanup as failure. This task and G3 remain unaccepted.
- [ ] **IN PROGRESS — P4.2 Add happy_buy.** Keep Alice seller/Bob buyer while reversing maker/taker creation. Follow
  the buy-order prerequisites: Alice pays the seller hold first, then maker Bob submits the buyer payout
  invoice. Share final settlement assertions without forcing the sell-order invoice sequence or duplicating
  platform-specific flow code.
- [ ] **TODO — P3.6 Accept both Linux happy paths from fresh actors.** Run happy_sell and happy_buy as
  separate invocations; record correlated evidence and complete cleanup under distinct run IDs.
- [ ] **TODO — P3.7 Accept both happy paths on Web.** Run the same business assertions, with their correct
  sell/buy invoice ordering, against fresh Web actors in separate invocations.

**Gate G3:** happy_sell and happy_buy each pass on Linux and Web with verified exactly-once payments and
clean teardown after active G1/G2 acceptance. **Not passed: Linux 0/2, Web 0/2.** Complete the relevant
verification and review documentation, then stop for operator review; do not start the deferred cases.

### 17.6 Phase 4 — Core trade and cancellation journeys (deferred)

**Status:** pending. **Primary files:** app-contract types/adapter/capabilities, v2 screen adapters,
`scenarios/src/smoke/`, `scenarios/src/regression/`, `scenarios/src/flow.rs` and corresponding app
screens/widget tests.

- [ ] **TODO — P4.1 Extend typed domain actions only where needed.** Introduce explicit fixed/market/range
  drafts, selected take amount, rating and cooperative-cancel outcomes. Keep browser/desktop details outside
  this API. Test serialization/validation, caps and normalized state behavior.
- [ ] **TODO — P4.3 Add cancel_before_taken.** Preserve existing cancel_pending compatibility if needed, but
  register the source name only once for counting. Assert terminal UI/book removal and no payment.
- [ ] **TODO — P4.4 Add cooperative_cancel and unilateral_cancel_rejected.** Expose explicit request/accept UI
  actions and counterpart notification/state; assert absence of premature cancellation and prove refund during
  cleanup.
- [ ] **TODO — P4.5 Add full_trade_with_rating.** Add stable rating controls and accepted-state readouts in
  app v2, submit each rating once and verify the source behavior through visible feedback and available public
  reputation data.
- [ ] **TODO — P4.6 Add range_order.** Add accessible range/take amount controls, validate min/max, observe
  the remainder as a distinct order and register its cleanup. Budget the taken amount without treating the
  range maximum as a completed payment or ignoring residual orders.
- [ ] **TODO — P4.7 Add market_price_order.** Read the resolved amount before payment, apply the existing
  cap/fee policy and fail safely if the live quote exceeds the budget. Record the observed quote instead of
  hardcoding a rate.
- [ ] **TODO — P4.8 Validate the phase set on Linux and Web.** Fresh actors per scenario, no inherited
  outcomes, evidence entries and clean order/payment state after every case. Review implementation and app
  contract commits.

**Gate G4 (deferred):** happy_sell, happy_buy and the other six core source scenarios pass on both targets.
This future gate is not required before the current operator review.

### 17.7 Phase 5 — Expiration and dispute journeys (deferred)

**Status:** pending. **Primary files:** new focused expiration/dispute scenario modules, scenario-kit
deadline/negative-observation helpers, app v2 expiry and dispute UI, v2 adapter actions/states and relay
dispute-event scope.

- [ ] **TODO — P5.1 Define declared daemon deadline inputs.** Load pending, buyer-invoice and hold-invoice
  limits plus sweep margin without deriving business behavior from arbitrary local sleeps. Validate scenario
  budgets before startup and capture observation cursors immediately before triggering UI actions so fast
  daemon responses cannot be missed. Negative-observation windows start after action acknowledgement.
- [ ] **TODO — P5.2 Add pending_order_expires.** Expose expiry selection through normal app UI and stable
  identifiers, submit the intended deadline, and assert expiry/book disappearance. Do not mutate the order
  through the runner.
- [ ] **TODO — P5.3 Add cancel_after_take_unpaid.** Abandon a take, wait for the declared recovery transition
  and validate whether the actual daemon returns pending, canceled or expired; distinguish the event from
  initial publication.
- [ ] **TODO — P5.4 Add hold_invoice_expires and buyer_invoice_deadline.** Make their prerequisites distinct:
  valid buyer invoice with unpaid seller hold versus absent buyer invoice. Assert no settlement and legal UI
  recovery.
- [ ] **TODO — P5.5 Prove dispute cleanup capability before funded tests.** Add missing public controls for
  request/accept cancellation of an untaken dispute, test state mapping and refund behavior. Refuse unsafe
  dispute cases before payment if a supported exit cannot be established.
- [ ] **TODO — P5.6 Add dispute_by_buyer and dispute_by_seller.** Preserve actor role and fiat-sent
  preconditions, correlate dispute ID with the public event, then unwind using the proved UI route without an
  imported solver identity.
- [ ] **TODO — P5.7 Add dispute_then_cooperative_cancel.** Assert both order and dispute terminal outcomes
  plus refunded hold. Where replaceable events share timestamps, use observed-event sequence/time predicates
  instead of sleeps or weakening assertions; fix daemon behavior if current evidence requires it.
- [ ] **TODO — P5.8 Validate all seven additions on both targets.** Record actual elapsed deadlines, cleanup
  outcomes and distinct source names. Review app, harness and any necessary daemon changes independently.

**Gate G5:** fifteen priority scenarios cumulatively accepted per platform; no open disputes or unresolved
escrow. **Not started.**

### 17.8 Phase 6 — Invoice faults and the complete twenty (deferred)

**Status:** pending. **Primary files:** wallet boundary/adapters, observers, app-contract capabilities, new
payment-failure scenarios and v2 invoice UI.

- [ ] **TODO — P6.1 Extend the regtest wallet fault boundary.** Provide typed create-with-expiry,
  cancel-invoice, create/settle/cancel-hold-invoice and observe-HTLC operations with injected fakes. Restrict
  mutations to tracked run-owned hashes and preserve real wallet-network and budget guards.
- [ ] **TODO — P6.2 Add invalid_buyer_invoice.** Separate local syntax validation from actual daemon rejection
  using syntactically valid invalid invoices where needed. Keep the order invoice-required and prove no
  illicit payment.
- [ ] **TODO — P6.3 Add payout_invoice_below_the_expiry_floor.** Generate an invoice below the declared floor
  with adequate test timing, submit through UI and verify the expected rejection rather than an unrelated
  parse error.
- [ ] **TODO — P6.4 Add payment_fails_on_release.** Invalidate the tracked payout invoice after fiat-sent and
  before release, matching the source. Release through the seller UI, verify failure/retry/replacement
  feedback, then recover and reconcile sats.
- [ ] **TODO — P6.5 Add buyer_invoice_expires_mid_flight.** Issue an invoice with the declared expiry floor
  plus submission margin, prove its acceptance, then invalidate it before release as the source does. This
  differs from P6.4 in invoice-lifetime construction, not cancellation timing. Replace it through UI and prove
  only the replacement settles.
- [ ] **TODO — P6.6 Add payout_slots_capped_per_destination.** Require explicit destination cap, start with no
  run-owned pending payouts, create enough ordinary UI trades using controlled hold invoices, assert the extra
  payout has no HTLC, free a slot and prove automatic dispatch without another release. Clean all
  orders/invoices and budget the entire group before funding.
- [ ] **TODO — P6.7 Register and audit ortsom-core-20.** Test that the tag maps to exactly the twenty source
  names in section 10. Reject alias double-counts; list capability failures explicitly and never convert them
  into passes.
- [ ] **TODO — P6.8 Run the complete Linux suite from fresh actor state.** Fill every Linux ledger cell with a
  passing run and clean teardown evidence.
- [ ] **TODO — P6.9 Run the complete Web suite.** Fill every Web ledger cell with the same business assertions
  and failure-injection points.

**Gate G6 (deferred):** twenty distinct priority sources pass per platform with no skips or unresolved
cleanup. Not part of this checkpoint; current two-case acceptance is Linux 0/2 and Web 0/2.

### 17.9 Phase 7 — Verification and delivery; resource measurements deferred

**Status:** pending. **Primary files:** a Rust resource-sampling boundary, coordinator lifecycle/report
integration, tests, README/runbooks and `docs/resource-comparison.md` (to be created).

- [ ] **TODO — P7.1 Implement resource sampling.** Read owned process trees and container counters through
  injectable readers/clock; preserve process start identity against PID reuse. Aggregate deltas into CPU
  seconds, mean cores, sampled RSS peak and optional PSS. Unit-test counter reset and vanished child handling;
  report missing samples rather than substituting zero.
- [ ] **TODO — P7.2 Add lifecycle measurement boundaries.** Record build, startup, ready idle, scenario
  execution and cleanup separately. Include driver/server overhead and distinguish shared stack cost from
  actor client cost.
- [ ] **TODO — P7.3 Execute matched repeated measurements.** Follow section 14: three full passing suites per
  platform, alternating order, identical inputs and immutable daemon/app artifacts. Exclude failed runs from
  comparison while retaining their diagnostic reports.
- [ ] **TODO — P7.4 Publish the local comparison artifact.** Produce an English report with median/range
  tables, reproducible commands, raw sample paths, hardware/tool versions and explicit RSS/shared-page and
  sampling limitations. Do not assert Android savings without a comparable existing baseline.
- [ ] **TODO — P7.5 Complete verification and coverage.** Run all required Rust checks and the >=80% line
  coverage gate, plus relevant app Flutter/Rust checks. Close desktop lifecycle, redaction, state and budget
  regression gaps.
- [ ] **TODO — P7.6 Finish operator documentation.** Document actual working setup/build/run/reset/cleanup
  commands and dependencies, supported platform capabilities, failure diagnosis and unresolved-escrow
  handling.
- [ ] **TODO — P7.7 Review completed commits and audit acceptance.** Independent review must confirm every
  checklist/ledger claim against code and evidence. Commit only completed tasks after checks/review; report
  the final source revisions and remaining limitations without claiming unfinished scope.

**Current delivery gate:** section 1.1 and the relevant P7.5–P7.7 checks/reviews pass, then stop. P7.1–P7.4
and their full-migration comparisons are deferred; do not execute extra paid runs after the checkpoint.

## 18. Current implementation evidence

Snapshot as of 2026-09-08; this is a progress record, not phase acceptance:

- The persistent Ortsom regtest stack is running. The pulled `latest` image
  was identified as mostrod v0.18.7, digest `sha256:bff406d11fc90436c9f7da5186f5976a63ae7ee1d711a6636d36205421cbda31`.
  This is the observed development image, not a permanent definition of latest.
- Real Mortsom `stack up` passed immutable-image/ownership/readiness verification. Ortsom commit `6353922`
  fixes the advertised host relay address. Read-only doctor passed 21 Linux and
  17 Web checks after stack/config binding and regtest wallet-capability validation; artifact provenance and full failure-path
  validation remain pending. Declared limits are imported; scenario verification of behavior remains pending.
- Linux and Web v2 builds and isolated read-only accessibility smoke passed.
- Both platforms passed real Alice/Bob onboarding, node/relay configuration
  and public identity persistence checks during happy_sell attempts.
- **No live trade scenario has passed yet.** Linux reached order submission
  without confirmation; its screenshot showed input values had not been
  accepted. Web reached an offscreen sats field that WebDriver could not edit.
  These are active failures, not accepted replications.
- A subsequent isolated Linux form diagnostic passed public readback of fiat amount, payment method and
  fixed sats after bounded focus/write retries. It canceled the form without publishing and released the
  client resources. It does not count as a trade or settle the remaining submission/cleanup requirements.
- Private captured-bundle launch diagnostics passed on Linux and Web, including visible test marker,
  onboarding skip, order-book control, client/server shutdown and captured-file removal. These single-client
  probes created no orders or payments. They do not validate recovery or the twenty scenarios.
- Source-build doctor passed 38 Linux and 35 Web checks after correcting the CLI syntax of the libsecret
  minimum-version check. It made no order or payment. Logs: `/tmp/mortsom-linux-source-doctor-recheck.log`
  and `/tmp/mortsom-web-source-doctor.log` (local diagnostic evidence, not committed artifacts).
- Workspace tests passed after native focus and bundle/server ownership fixes. The subsequent offline
  maintenance changes passed all 167 coordinator tests and seven CLI integration tests, plus full workspace
  strict clippy and formatting. Logs: `/tmp/mortsom-offline-maintenance-recheck.log` and
  `/tmp/mortsom-maintenance-clippy.log`. Independent review closed recorded-platform and publication
  compatibility findings. New-driver coverage, lifecycle failure tests, complete build provenance and
  per-scenario reset remain pending.
- Run preparation now rejects missing host/build tools before invoking app generation. A real CLI probe
  with a restricted PATH exited 2 naming Flutter and Xvfb, without clients/orders/payments; evidence is
  `/tmp/mortsom-prebuild-live-missing-tools.log`. Relative and absolute Xvfb overrides have regressions.
  Doctor's read-only call graph was independently audited. Backend-supported LND operations are reported;
  the probe does not exercise or prove mutation permissions of the configured macaroon.
- Independent review found startup-readiness and cleanup gaps. Fixes are in
  progress and are not considered accepted until tests and live reruns pass.
- Source build/capture provenance passed on both targets with app version 2.0.0. Logs:
  `/tmp/mortsom-linux-provenance-build.log` and `/tmp/mortsom-web-provenance-build.log`. Adding the Git
  prerequisite changes future source doctor totals to 39 Linux / 36 Web; those updated totals await a rerun.
- Diagnostic run `20260908T141446Z-8f0617eeb9de` created sell order
  `35ec3491-7362-4d58-ac84-2b8e992e7f11` through Linux UI, then failed on a trade-list navigation race during
  cancellation. All payment/reservation totals were zero; unresolved cleanup correctly failed the run.
  All six persistent containers survived unchanged and both clients shut down. The current driver deleted
  profiles, demonstrating the remaining P2.12 recovery gap. An explicit regtest down/up discarded this
  unfunded order; this was environment recovery, not a UI cancellation or scenario pass. The new stack
  has a new daemon identity and requires new builds. The submitted order's observed expiry was 3600 seconds,
  so the declared 300-second stack value must not be treated as that UI order's actual expiry.
- Metadata verification passed 179 coordinator and seven CLI tests; independent review closed the source
  toolchain context and fresh-directory durability findings. Log:
  `/tmp/mortsom-directory-durability-green.log`.
- Bounded Web field editing and value readback passed the original offscreen fixed-sats form step, plus
  fiat and method fields. Native readback passed the same probe. Both created no orders or payments and
  released their client/server/bundle resources. Logs: `/tmp/mortsom-web-form-readback-live.log` and
  `/tmp/mortsom-linux-form-readback-live.log`. Fifteen desktop tests passed, including stale and
  non-interactable element retries, ignored edits and input redaction. These are form probes, not trades.
- Ordinary lifecycle validation passed 531 workspace tests; the later cancellation-safe server join passed
  23 desktop tests and four coordinator factory tests, plus full strict clippy. Both real copied-bundle form
  probes passed again with no submission. Logs: `/tmp/mortsom-lifecycle-workspace-tests.log`,
  `/tmp/mortsom-server-cancel-join-tests.log`, `/tmp/mortsom-desktop-factory-final-tests.log`,
  `/tmp/mortsom-linux-final-lifecycle-live.log` and `/tmp/mortsom-web-final-lifecycle-live.log`.
  These prove bounded in-process cleanup of owned groups; durable unresolved profiles remain unfinished.
- The current two-case Linux/Web matrix remains unaccepted. The other eighteen cases and CPU/RSS
  comparison are deferred until operator review, per the explicit scope change on 2026-09-08.

The maintained per-scenario evidence index is [Ortsom UI parity](docs/ortsom-ui-parity.md). Update this status
when verified results change; never replace failed evidence with an unqualified success claim.

### 18.12 Current reduced-checkpoint work (2026-09-08)

The current target remains four accepted cells only: happy_sell/happy_buy on Linux/Web. None is yet accepted.
The source builds have been refreshed against the current stack after the explicit regtest reset. Linux
compiled the source later committed as app `e83cd8f`; Web compiled that clean commit. Source-build logs:
`/tmp/mortsom-checkpoint-linux-rebuild.log` and `/tmp/mortsom-checkpoint-web-rebuild.log`.

Real cross-handle recovery passed with no orders/payments on both targets:

| Target | Recovery probe run | Verified outcome |
|---|---|---|
| Linux | `20260908T154215Z-53d3093b0ef8` | Both original public identities restored with new sessions; all owned processes stopped; profiles/bundle removed |
| Web | `20260908T154752Z-b4c8168f49a0` | Both original public identities restored with new browsers and the exact original server origin; complete cleanup |

Logs are `/tmp/mortsom-linux-recovery-smoke.log` and `/tmp/mortsom-web-recovery-smoke.log`. Private local
result evidence is under `$HOME/.local/state/mortsom/recovery-probes/evidence/<run>/result.json`. The probes
exercise ordinary UI identity readback, never private app state. They are G2 evidence, not trade passes.

Durable coordinator recovery is under final review. A private, atomically replaced journal records actor
ownership, captured bundle identity, original origin and pre-action order/payment intents. UI-only cleanup
reopens original identities against the original stack. Four coordinator regressions cover actual reopened
journals, failed then successful cleanup, identity mismatch and changed-stack refusal. Missing/changed
storage fails closed. Payment recovery must prove exact amounts and one actual settled ledger row per hash;
repeated observations and MPP attempts are not duplicate settlements. Unused tracked buyer invoices are
canceled through the dedicated run-owned LND wallet, with a verified canceled state before deleting profiles.

The desktop checkpoint requires both external LND wallets; NWC desktop recovery is deferred. Desktop
infrastructure failures do not automatically retry a scenario with the same identities. Pre-submit write
failure refuses creation, and a confirmation timeout never republishes the same order. Forty driver tests
cover protected ownership, runtime markers, shutdown failures and typed native retries. Four captured-bundle
tests cover owner-drop/resume and changed-content refusal. A durable active-runtime marker survives abrupt
termination: SIGKILL recovery refuses to reuse or delete profiles until the owned processes have been
reconciled. Automatic reconciliation of a killed coordinator's orphaned processes is not implemented.

LND observation now includes in-flight updates so the hold-payment step can proceed before release. Exact
principal accounting rejects fractional sats, reserves actual escrow plus the enforced routing-fee cap, and
rounds fees upward. Seventy-five environment tests and strict environment clippy passed. Happy-buy has four
injected flow regressions; full workspace tests passed before the latest recovery additions. Final checks,
coverage, review and all four live happy-path cells remain pending.

The final review also exposed failure-path gaps: pre-payment reservation leaks, removal of pending hold
ownership before bounded settlement observation, and teardown artifact-write errors that were only logged.
Corrections and regressions are in progress; these do not change the four pending acceptance cells.
A payment intent durably recorded just before a call remains unresolved if the coordinator stops before
learning whether the payment started. Unknown wallet state is never interpreted as proof of a refund or
absence of spend; this deliberately retains profiles for reconciliation.

### 18.13 Implementation map and immediate remaining work

| Task | Implemented files / behavior | Verified so far | Remaining acceptance |
|---|---|---|---|
| P2.5 | `desktop-driver/native.rs`, `native_retry.rs`: typed stale-object retries for reads/focus/text; business taps are never replayed after invocation | Native regression tests and real Linux form readback | Final combined checks/review |
| P2.11 | App v2 `e83cd8f`; coordinated semantic names, sidebar, payout state/polling and manual invoice readouts | Full app checks and independent commit review passed | No outstanding implementation task; trade behavior still needs G3 |
| P2.12 | `desktop-driver/storage.rs`, Linux/Web lifecycle: private Fresh/Resume profiles and durable active-runtime markers | Forty driver tests; both real two-actor cross-handle recovery probes | Final combined checks and commit review |
| P2.12 | `coordinator/recovery_store.rs`, `fingerprint/capture.rs`, `run/recovery/`, `cli/recover.rs`: atomic journal, persistent captured bundle, exact identities/origin/stack, UI cleanup and verified deletion | Reopen/tamper/write-failure tests; ten coordinator recovery regressions including payout-pending completion, duplicate settlement refusal and Bob-owned orders | Final evidence-write tests, combined checks and review |
| P3.1–P3.2 | V2 `screens/orders.rs`, `scenario-kit/actions.rs`, `journal.rs`: form readback, durable submission intent, a single submit, public discovery after uncertainty | Real form probes; write-failure and single-publication regressions | Successful creation/discovery in the four target runs |
| P3.3–P3.4 | `actions/buyer_invoice.rs`, `actions/hold.rs`, `environment/lnd/`, `run/recovery/payments.rs`: exact invoice amount/hash, bounded in-flight observation, escrow-plus-fee reservation, terminal UI and ledger verification | Invoice/readback tests, 75 environment tests, recovery payment regressions | Final hold ownership regression and all live funded paths |
| P3.5 | `run/execute.rs`, `world_execution.rs`, `run/report.rs`: payout-pending wait, truthful cancellation, protected retention, public cleanup status, fail-closed evidence writes | Reopened cleanup and identity/stack mismatch regressions | Evidence-write failure regression, live cleanup and final review |
| P4.2 | `scenarios/src/smoke/happy_buy.rs`, shared `flow.rs`: Bob creates buy, Alice funds first, Bob invoices next | Four injected flow tests including wrong ordering/cap/payment failures | Linux and Web happy_buy live passes |
| P7.6 | README desktop quickstart and runbook recovery instructions | Commands checked against current CLI/config | Confirm commands against four passing live runs |

Next, in order: finish the two outstanding failure-path regressions; run formatting, workspace clippy and
relevant tests; close G2 with independent review; run happy_sell and happy_buy separately on Linux, then
on Web, fixing failures and preserving their evidence; finish coverage/reviews and record the four successful
run IDs. Stop at that checkpoint. No deferred scenario or resource-benchmark run is authorized by this plan.

Final combined verification for G2 passed: `cargo fmt --all`,
`cargo clippy --workspace --all-targets -- -D warnings`, `cargo test --workspace --all-targets`
(604 passed, zero failed, one existing ignored test), and the coordinator binary build. Logs are
`/tmp/mortsom-checkpoint-final-{clippy,tests,build}.log`. Independent native/storage, coordinator recovery,
app commit and evidence-write reviews found no remaining blocker for G2. Prepayment and pending-hold
ownership regressions now pass; ambiguous NWC UI errors also retain InFlight budget because they do not
prove no payment occurred. This is an accounting safety correction, not Android feature work.
