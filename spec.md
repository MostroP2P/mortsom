# Mortsom — Technical Specification

> End-to-end test harness for the Mostro mobile application.
> Mortsom is *Mostro Mobile* written backwards: it watches a real trade from
> the user's side of the screen.

**Version:** 0.1 (draft)  
**Status:** design proposal  
**Language policy:** English is used for code, comments, identifiers,
documentation, commit messages, issues, logs, and error strings.

**Current target application:** `MostroP2P/app`, the second-generation
Flutter/Rust client (local checkout `../mobilev2`). Linux is the default
execution target; Web is the alternative. Android remains selectable but
its existing implementation is frozen during the resource comparison.

### September 2026 execution amendment

This amendment supersedes the Android-only topology, legacy-first ordering,
and per-run relay ownership described below. Those sections document the
existing implementation and still apply to the frozen Android path.

The operator first runs `ortsom stack up` in the sibling checkout. Ortsom
owns the persistent regtest chain, three LND nodes, relay and daemon. Mortsom
may invoke that CLI through `mortsom stack up`; it must not import Ortsom's
runner or copy its source. Stack lifecycle and scenario execution are separate
commands. A scenario run attaches to the configured stack and never removes
its containers. Explicit stack cleanup is performed through the Ortsom CLI.

`mortsom run --platform linux|web|android` selects the same target for Alice
and Bob, defaulting to Linux and the v2 profile. Linux actors have independent
processes, XDG storage and accessibility sessions. Web actors have independent
browser profiles and storage, driven through the rendered Flutter semantics
DOM using W3C WebDriver. Linux uses the public desktop accessibility API.
Neither driver may invoke application internals or advance orders outside UI.
The app owns any required accessibility contract additions.

The Web build includes the app's supported Rust/WASM build and cross-origin
isolation headers. Both targets use the same daemon identity and local relay,
with production fallback disabled. Existing lane selection, secret handling,
caps, wallet accounting and cleanup requirements remain mandatory. Regtest
clearance requires observed wallet networks; a local hostname is insufficient.

Implementation gates for this migration are sequential:

1. Attach to a ready Ortsom stack without mutating its lifecycle during a run;
   validate platform selection and configuration with fakes and CLI tests.
2. Launch two isolated v2 clients and prove onboarding and identity persistence
   through accessibility on Linux, then Web; clean partial startup reliably.
3. Complete `happy_sell` with UI, relay and wallet evidence and clean teardown.
4. Replicate at least 20 distinct, important Ortsom scenarios through the app's
   available UI. Document each source scenario, assertions and actual result.
   Unsupported features, skipped cases and fake-driven tests do not count as
   successful live replications. Fix failures and rerun from clean actor state.
5. Record comparable CPU/RSS measurements for Linux and Web on the same suite,
   and independently review the implementation commits.

---

## 1. Decision and scope

Mortsom is a new repository. It is not a crate inside Ortsom.

Ortsom tests the Mostro daemon and its public protocol by driving two
protocol-native clients. Its core abstractions are Nostr messages, Lightning
payments, orders, and observed daemon state. Mortsom initially tests the
classic Flutter/Dart Android mobile application by driving two app
installations like real users. Its core
abstractions are screens, accessibility semantics, navigation, lifecycle,
notifications, external-wallet handoff, and visible order state.

Combining them would make protocol tests depend on Android tooling and UI tests
depend on daemon-oriented actor APIs. That would blur the system under test,
increase CI cost and flakiness, and make mainnet safety controls harder to
audit. The projects should share concepts and selected artifacts, not source
code or a runner.

Mortsom is complementary to Ortsom:

- Ortsom asks whether a compliant client can complete a Mostro flow.
- Mortsom asks whether a human can complete and understand that same flow in
  the mobile app.
- A failure seen by both projects points toward the daemon or protocol.
- A failure seen only by Mortsom points toward the app, its integration, or UX.

## 2. Goals

Mortsom must:

1. Test release binaries of the mobile app as black boxes.
2. Operate Alice and Bob on independent Android devices or emulators.
3. Execute realistic order journeys through the rendered interface.
4. Verify UI states and correlate them with relay, wallet, and daemon evidence.
5. Test recovery from interruption: restart, backgrounding, lost connectivity,
   stale events, and wallet cancellation.
6. Produce sanitized artifacts sufficient to diagnose a failure.
7. Support a fast PR suite, a broader scheduled suite, and a tightly limited
   mainnet canary.

Mortsom is not a substitute for app unit/widget tests, protocol conformance,
load testing, or manual usability research. It never uses personal keys or
wallets, and it never silently runs a value-moving scenario against mainnet.

## 3. Black-box contract

The acceptance suite may observe only what a user or the test environment can
legitimately observe:

- Android accessibility tree, visible pixels, app process state, and system
  notifications;
- Nostr events on relays controlled by the run;
- invoices and payments for wallets dedicated to the run; and
- health/readiness signals from controlled infrastructure.

It must not read the app database, invoke private application methods, mutate
orders through an admin endpoint, or assert Flutter implementation details.
The app may provide narrow **test-environment bootstrap** support: choose an
environment before first launch, inject a test identity during onboarding, and
select a supported test wallet. It must not provide test-only commands that
create, take, pay, release, cancel, or alter an order outside the UI.

Stable accessibility IDs are a product contract. Every actionable control and
business-critical state needs one. Selectors may not use absolute coordinates,
hierarchy depth, text fragments likely to be localized, or image matching as
the normal mechanism.

## 4. System topology

```
                         ┌─────────────────────────┐
                         │        Mortsom          │
                         │ runner · cleanup ·      │
                         │ budget · artifacts      │
                         └──────┬──────────┬───────┘
                                │          │
                   ┌────────────▼───┐  ┌──▼───────────────┐
                   │ Alice Android  │  │ Bob Android      │
                   │ Mostro Mobile  │  │ Mostro Mobile    │
                   └──────┬─────────┘  └────────┬─────────┘
                          │ UI automation         │
             ┌────────────▼──────────────────────▼──────────┐
             │ Android driver: accessibility, screenshots,   │
             │ notifications, lifecycle, network conditions  │
             └────────────┬──────────────────────┬──────────┘
                          │                      │
            ┌─────────────▼──────┐    ┌──────────▼──────────┐
            │ controlled relays  │    │ dedicated wallets    │
            └─────────────┬──────┘    └──────────┬──────────┘
                          │                      │
                    ┌─────▼──────────────────────▼──────┐
                    │ Mostro test daemon and LND/node     │
                    └─────────────────────────────────────┘
```

### 4.1 Runner

The runner selects scenarios, allocates device/identity slots, starts and
checks the environment, enforces timeouts and spend limits, collects evidence,
and performs teardown. It runs scenarios concurrently only when their devices,
identities, app storage, wallets, and orders are isolated. Steps inside one
scenario remain strictly sequential.

### 4.2 Android automation

Mortsom is implemented entirely in Rust. Its production Android driver uses a
Rust WebDriver client to control an Appium server with UiAutomator2. This keeps
the runner, configuration, safety checks, observers, artifacts, and adapters
in one Rust codebase while retaining Android's accessibility interface and the
ability to automate native dialogs and external-wallet handoffs. Appium is an
external automation service, not a JavaScript or TypeScript implementation
dependency of Mortsom.

The driver API is domain-oriented. A scenario calls `create_fixed_sell_order`,
not `tap("x=312,y=842")`. The driver translates a domain action into a small
screen-object sequence and records an evidence checkpoint after it completes.

### 4.3 Evidence observers

Observers are read-only evidence sources:

- **UI observer:** screenshot and accessibility dump at each meaningful step.
- **Notification observer:** records delivery and verifies that a notification
  opens the correct order.
- **Relay observer:** records events for run identities and order IDs.
- **Wallet observer:** records invoice/payment hashes, amounts, fees, and
  terminal states for dedicated wallets.
- **Infrastructure observer:** records relay, daemon, emulator, and wallet
  readiness, not private application data.

## 5. Environment lanes

Mortsom uses three explicit lanes. A lane must be named in configuration and
in the final report; a generic `test` command may not infer mainnet.

| Lane | Purpose | Value/network | Default use |
|---|---|---|---|
| Local deterministic | authoring and debugging | local relay, test daemon, controlled wallet adapter | developer machine |
| CI integration | release confidence | isolated relay and dedicated test infrastructure | every PR smoke suite |
| Mainnet canary | real-world integration detection | dedicated daemon/wallets, tiny hard-capped amounts | scheduled only |

The local lane may simulate Lightning success, failure, or a held payment. It
is valid for UI behavior but not a claim of Lightning interoperability. Payment
settlement claims belong in CI integration or the canary. The canary inherits
Ortsom's safeguards: dedicated wallets and identities, fixed low amounts,
per-order and per-run caps, pre-flight liquidity checks, cancellation cleanup,
and accounting. A canary failure requires review; it must not loop retries that
could spend more funds.

## 6. Bootstrap, identities, and wallets

Every live scenario gets a separate Alice/Bob identity slot and fresh app
storage. Test mnemonics are injected once during onboarding through a narrow,
build- or launch-time bootstrap interface. They come from secret storage, never
from a committed config, log, screenshot, or normal artifact. The app then acts
normally; the runner does not bypass UI actions.

The first vertical slice targets one supported wallet journey only. Two wallet
adapters are expected eventually:

- **NWC adapter:** configures wallets controlled by the test environment.
- **External-wallet adapter:** installs a companion test receiver for the
  wallet intent/deep-link callback and can return success, cancellation, or
  failure through the same public path used by a real wallet.

Supporting all wallet providers first would multiply flakiness. Each additional
provider is a capability with its own scenario tag and acceptance contract.

## 7. Scenario model

Scenarios are typed domain narratives, not long tap scripts. A step finishes
only after its expected visible state appears and, when appropriate, its
external consequence is observed. There are no fixed sleeps. A wait is always
a bounded predicate over UI state, notification, relay event, wallet callback,
or an explicit negative-observation interval.

```text
scenario happy_sell:
  alice.launch_clean()
  bob.launch_clean()
  alice.create_fixed_sell(amount, fiat, payment_method)
  expect alice.order_status("Pending")
  expect relay.order_visible(order)
  bob.find_and_take(order)
  expect both.order_status("Waiting for seller")
  alice.pay_hold_invoice_via_wallet()
  expect both.order_status("Active")
  bob.confirm_fiat_sent()
  expect alice.order_status("Fiat payment reported")
  alice.release()
  expect both.order_status("Completed")
  expect wallet.payment_settled(order)
```

Each step declares actor, precondition, stable selector(s), user-visible
success state, correlation requirement, timeout, artifact checkpoint, and
cleanup behavior. Cleanup is registered as soon as an order could exist.

## 8. Assertions

UI assertions verify what the user can see and do: screen identity,
accessibility name, enabled/disabled action, normalized monetary values, order
status, validation or error category, list membership, navigation destination,
and notification routing.

Correlation assertions verify that a visible claim is grounded:

- displayed order identity maps to a run-scoped relay observation;
- a terminal status maps to a known wallet terminal state;
- a notification opens the matching order, not just any order screen;
- app restart reconstructs the same order state without duplicating an action;
- rejected actions produce neither an illegal UI transition nor an illegal
  relay event.

Correlation is eventual but bounded. Brief network/UI lag is acceptable until
the step deadline; a false completed state is not. Where the product uses an
intermediate state, its copy must say so clearly and Mortsom asserts that state
instead of prematurely asserting settlement.

## 9. Scenario catalogue

### 9.1 PR smoke suite

1. First launch, onboarding, test-environment selection, and identity import.
2. Create a fixed-price sell order and find it in history.
3. Discover and take that order from a second device.
4. Complete a sell flow through the first supported wallet path.
5. Complete a buy flow through the same path.
6. Cancel a pending order and verify removal on both devices.
7. Relaunch while an order is pending, taken, active, fiat-reported, and
   completed; state must be restored correctly.

### 9.2 Scheduled regression suite

- Required-field, amount, range, currency, and localized-number validation.
- Loading, empty, offline, and reconnect states.
- Wallet cancellation, invalid invoice, payment failure, and retry behavior.
- Expiration copy and resulting state for both participants.
- Ratings, history, and completed-order details.
- Notification receipt and deep-link routing.
- Backgrounding, force-stop, device rotation, and network loss at each durable
  state.
- Multiple concurrent orders on one device; no action may affect another order.
- Dispute initiation and solver-resolution presentation.

### 9.3 Adversarial UX suite

- Delayed, duplicate, stale, and out-of-order events must not create duplicate
  cards, enable illegal actions, or regress terminal state.
- Crafted deep links must not grant an unrelated identity control of an order.
- Reinstall/account switch must not silently reuse an old account context.
- Sensitive data must not appear in logs, screenshots, Android recents, or
  error messages.

## 10. Reliability and flake policy

Device pairs, identities, wallet credentials, and app storage are exclusive to
a live scenario. CI uses pinned emulator images and controlled relays. Emulator
snapshots reset only between scenarios, never during a scenario.

Before a run, Mortsom checks device availability, app install/version,
environment reachability, relay readiness, daemon readiness, and wallet
funding where relevant. Failure categories are explicit: product failure,
automation-contract failure, test-infrastructure failure, relay failure,
wallet failure, and environment misconfiguration.

One retry is allowed only for an explicitly classified infrastructure failure.
UI assertion failures, state-correlation mismatches, and payment mismatches may
not be retried into a pass. The runner records retries, duration, and category
per scenario. A quarantined scenario requires an issue, owner, and expiry date.

## 11. Artifacts and privacy

Each run yields:

```text
artifacts/<run>/
  summary.json
  timeline.md
  environment.json
  alice/steps/<step>-screen.png
  alice/steps/<step>-ui.xml
  bob/steps/<step>-screen.png
  bob/steps/<step>-ui.xml
  notifications.jsonl
  relay-events.jsonl
  wallet-payments.json
  device-logcat.txt
  cleanup.json
```

Artifacts are sanitized before upload. Mnemonics, private keys, NWC URIs,
macaroons, tokens, sensitive invoices, and fiat-payment details must be
redacted. Screenshots pass through redaction before CI publication. Raw
evidence, if retained at all, stays in the secured runner for a short,
documented period and is accessible only to authorized maintainers.

## 12. Implementation shape

Use Rust exclusively for Mortsom. The coordinator, configuration, Docker and
emulator lifecycle, WebDriver client, relay/wallet observers, scenario kit,
adapters, reporting, and command-line interface are Rust crates. Appium and
UiAutomator2 run only as external services reached through the WebDriver
protocol; Mortsom contains no TypeScript or Node application code.

```text
mortsom/
  spec.md
  docs/
  packages/
    coordinator/        # runner, budgets, timeouts, cleanup, reporting
    app-contract/       # shared domain actions, states, capabilities, fixtures
    app-adapters/       # one adapter per mobile application generation
      mobile-legacy/    # first adapter: ../mobile
      mobile-v2/        # future adapter: ../mobilev2
    android-driver/     # Appium/UIAutomator2 and emulator primitives
    environment/        # emulator, relay, daemon, and wallet provisioning
    observers/          # UI, notification, relay, wallet, artifacts
    scenario-kit/       # domain actions, assertions, shared models
  scenarios/
    smoke/
    regression/
    recovery/
    adversarial/
  config/
    local.example.yaml
    ci.example.yaml
    canary.example.yaml
  artifacts/            # ignored
```

The mobile app repository owns accessibility identifiers and bootstrap hooks.
Mortsom owns automation, scenario definitions, environment orchestration, and
reports. Changes to a semantic identifier, visible business state, or
external-wallet callback are contract changes and require coordinated review.

### 12.1 Multi-application adapter boundary

Mortsom's coordinator, Android lifecycle, Docker relay, daemon management,
NWC configuration, budget accounting, observers, artifacts, and scenario
language are application-independent. An `AppAdapter` is the only layer that
knows a particular app package, APK build procedure, bootstrap mechanism,
accessibility identifiers, navigation, and screen copy.

The adapter exposes business capabilities rather than widget details:

```text
AppAdapter
  build(profile) -> Apk
  install_and_bootstrap(device, actor, environment)
  configure_nwc(device, wallet_reference)
  create_sell_order(device, draft) -> VisibleOrder
  find_and_take_order(device, order_reference)
  pay_hold_invoice(device) -> PaymentHandoff
  confirm_fiat_sent(device, order_reference)
  release(device, order_reference)
  cancel_if_open(device, order_reference)
  observe_order(device, order_reference) -> AppOrderState
  capabilities() -> CapabilitySet
```

The shared scenario kit operates only on this contract and on normalized
business states such as `Pending`, `Taken`, `Active`, `FiatSent`, `Completed`,
`Cancelled`, `Expired`, and `Disputed`. It must not import a legacy selector,
Flutter route, Rust-core type, or localized string. An adapter maps these
states to its application-specific rendering and proves the mapping through
adapter contract tests.

### 12.2 Profiles and capability gating

Each application generation has a named profile, initially `mobile-legacy` and
later `mobile-v2`. A profile selects source directory, build command/package,
environment bootstrap implementation, selector registry, and supported
capabilities. The same scenario can run against both profiles when it requires
only their common capabilities.

For example, `happy_sell` requires onboarding, custom Mostro node selection,
local relay configuration, NWC, create/take, hold-invoice payment, fiat sent,
release, and completed-state observation. A scenario that requires a feature
not yet available in one app is reported as **skipped with a reason**, never
passed and never silently omitted. The report includes app profile and app
version for every result.

This avoids freezing Mortsom around the old app while avoiding a premature
lowest-common-denominator design. The legacy adapter is built first; the v2
adapter is added only after the shared contract has proven one full trade.

### 12.3 Cross-app parity suite

When both adapters exist, Mortsom adds a `parity` suite. It runs a shared
scenario against each profile in isolated runs and compares normalized outcome,
legal state path, relay correlation, wallet result, cleanup result, and
accessibility contract. It does not require pixel-identical screens or the
same internal architecture. Differences in labels, navigation, storage, and
implementation are adapter responsibilities; differences in user-visible
business behavior become an explicit product decision or a finding.

## 13. Open decisions

1. Which wallet integration is the first acceptance target?
2. Does the app already provide test-environment selection and safe identity
   bootstrap, or must those be implemented?
3. Which Android versions and device profiles constitute release support?
4. Is a controlled Lightning adapter acceptable in PR UI tests, with real-LN
   checks reserved for scheduled and canary lanes?
5. Where will protected replay secrets and raw artifacts reside?
6. Which user-visible order-state labels are contractual across releases?

## 14. Success criteria

The first release succeeds when it can automatically complete one trade between
two app instances, capture privacy-safe evidence, clean up after any failure,
and accurately classify a UI defect versus relay, wallet, daemon, or
infrastructure failure. The program is mature when its PR suite catches mobile
regressions without becoming ignored noise, and its canary detects
production-integration drift within a bounded amount of real value at risk.

---

## 15. Concrete environment contract for `mobile`

This section is normative for the first implementation. Mortsom targets the
existing Flutter application in `../mobile`, not a hypothetical generic app.
The runner receives the source checkout or a prebuilt APK, builds the APK when
needed, creates two clean Android virtual devices, and installs the exact same
APK on both devices.

### 16.1 Configuration model

One committed example file documents all non-secret defaults. One ignored local
file and environment variables provide secrets. Mortsom resolves every required
secret before creating a Docker container, booting an emulator, or starting a
wallet connection. It must list all missing variable names at once.

```toml
# mortsom.toml -- committed, non-secret baseline
[mobile]
source_dir = "../mobile"
flavor = "mortsom"
apk_path = ""                     # empty means build from source_dir
mostro_pubkey = "env:MORTSOM_MOSTRO_PUBKEY"
dart_defines = ["MOSTRO_PUB_KEY=${MORTSOM_MOSTRO_PUBKEY}"]

[android]
avd_image = "system-images;android-35;google_apis;x86_64"
device_profile = "Pixel_6"
api_level = 35
boot_timeout_secs = 180
ui_timeout_secs = 30
snapshot_name = "mortsom-clean"

[relay]
docker_image = "scsibug/nostr-rs-relay:latest"
container_name_prefix = "mortsom-relay"
host_port = 7000
container_port = 8080

[mostro]
mode = "external_local"           # a locally managed Mostro daemon
settings_file = "env:MORTSOM_MOSTRO_SETTINGS_FILE"
health_timeout_secs = 120
relays = ["ws://host.docker.internal:7000"]

[wallets]
mode = "nwc"
alice_nwc = "env:MORTSOM_ALICE_NWC"
bob_nwc = "env:MORTSOM_BOB_NWC"

[run]
amount_sats = 1000
max_order_sats = 5000
max_run_spend_sats = 20000
jobs = 1
artifacts_dir = "./artifacts"
cleanup = true
allow_mainnet = false
```

In the mainnet canary profile, `allow_mainnet` must be explicitly true, the
runner must require an additional confirmation environment variable, and all
spend caps remain mandatory. Mainnet NWC URIs, macaroons, seeds, and private
keys are secrets. Mortsom never writes them to TOML, shell history, artifacts,
logs, screenshots, or test reports.

### 16.2 Nostr Wallet Connect configuration

Alice and Bob each receive a different NWC URI. They must map to dedicated,
funded test wallets and must never refer to a personal or production wallet.
The runner validates each URI syntactically, redacts it immediately in memory
representations, checks reachability through the NWC protocol, and records only
an opaque wallet label and capability summary in the artifact.

The classic mobile app already exposes NWC support. The first implementation
must configure each URI through the normal settings UI. A test-only bootstrap
may prefill the URI behind a build flag only after a UI-driven configuration
scenario exists and proves that the settings page itself works. Even with a
prefill, the app must display a connected-wallet state before a payment
scenario begins.

The harness records the payment hash, requested amount, terminal outcome, and
fee for payments that belong to the scenario. It never relies on a global
wallet balance assertion: parallel or prior activity makes such assertions
ambiguous. A payment is associated with an order through its invoice and hash.

### 16.3 Local relay lifecycle

Mortsom owns the local relay lifecycle just as Ortsom does. For every run it:

1. Allocates a unique Docker project/container name and port, preventing a
   parallel developer run from attaching to the wrong relay.
2. Starts `nostr-rs-relay` through Docker with a run-specific data directory.
3. Waits for the relay WebSocket endpoint and performs a Nostr protocol probe.
4. Publishes or verifies the relay-list event expected from the test daemon.
5. Gives the emulator a reachable relay hostname. Android emulators use
   `10.0.2.2` for services bound on the host; Docker and host networking must
   be configured so both the app and daemon refer to the same relay endpoint.
6. Persists sanitized relay logs and event observations as artifacts.
7. Stops and removes the container and run data at teardown unless a failure
   explicitly requests retained diagnostics.

The runner must not silently fall back to public relays. The classic app has
bootstrap relays for discovery, so the dedicated Mortsom build/configuration
must prevent those production endpoints from changing the test's network
boundary. The test daemon's kind-10002 relay list must advertise the local
relay that both app instances can reach.

### 16.4 Local Mostro daemon with mainnet Lightning

The system under test is a locally operated Mostro daemon. It uses its normal
mainnet Lightning connection, as Ortsom does, while the relay and daemon
process are local. Mortsom does not start a fake daemon for end-to-end claims.

The pre-flight sequence is:

1. Read the daemon configuration path from a secret environment reference.
2. Verify that its configured pubkey equals `MORTSOM_MOSTRO_PUBKEY`.
3. Verify that its relay configuration contains the Mortsom local relay.
4. Verify daemon readiness and LND connectivity without creating an order.
5. Verify test timeout values: pending order, hold invoice, buyer invoice,
   fiat window, and cleanup sweep must be short enough for scenarios.
6. Verify the dedicated wallets have the minimum funding and that configured
   caps are lower than available funds.
7. Refuse a mainnet run unless all checks pass and explicit mainnet opt-in is
   present.

Mortsom may launch the daemon only when it owns a declarative local service
definition. Otherwise it treats the daemon as an external local dependency,
checks it, and never restarts an operator-owned process. The first phase should
use the latter mode; automatic daemon process ownership may be added after the
health and cleanup contract is proven.

### 16.5 Android emulator lifecycle

Each actor gets a distinct AVD name, Android user data directory, Appium
session, and app data directory. The runner must implement these operations:

1. Locate Android SDK, emulator, adb, Appium, UiAutomator2, Flutter, and Docker
   before beginning work; report missing prerequisites in `mortsom doctor`.
2. Create the pinned AVD image if absent, configure RAM/disk/GPU deterministically,
and create a clean snapshot after baseline boot.
3. Boot Alice and Bob in parallel, wait for Android boot completion, disable
animations, set locale/time zone, configure emulator network routing, and
unlock screens.
4. Install the exact APK/version on both devices and verify package/version
   identity before launching.
5. Start independent Appium sessions and verify accessibility queries, tap,
   text input, screenshot capture, app backgrounding, force-stop, and relaunch.
6. Clear application data and restore the clean emulator snapshot after each
   scenario. Never reuse a previous scenario's account, secure storage, or
   Sembast database unless the scenario explicitly tests persistence.
7. On failure, collect logcat, UI hierarchy, screenshots, Android process
   state, emulator configuration, and relevant host-side service logs before
   cleanup.

## 16. Required application-facing changes

Mortsom must not begin by automating unstable visual details. The `mobile`
repository needs an explicit automation contract, implemented and reviewed as
product code.

### 17.1 Build-time test configuration

The classic app already accepts `MOSTRO_PUB_KEY` through a Dart define. Extend
that pattern, without changing normal production behavior, to provide:

- a test build flavor that accepts the Mostro pubkey and local relay seed list;
- a way to disable public bootstrap-relay fallback in that flavor;
- an explicit test-environment marker visible in the app and report;
- optional onboarding-only test identity injection; and
- optional NWC prefill that is disabled by default and does not log a URI.

Values passed as Dart defines are visible to build tooling; secrets must not be
passed there. Pass only non-secret daemon identity and relay endpoints by Dart
define. Provide NWC URI and mnemonic at runtime through protected Appium input
or an emulator-local secret channel that is removed after onboarding.

### 17.2 Accessibility contract

Add stable semantic identifiers to onboarding, settings, NWC connection,
order-book, order-detail, create-order, take-order, invoice, fiat-sent,
release, cancellation, dispute, and error controls. Use identifiers instead of
localized labels. Each semantic identifier receives a short owner and a
behavioral contract in `mobile/docs/automation-contract.md`.

Important examples include `onboarding.import_mnemonic`,
`settings.nwc.connect`, `order.create.submit`, `order.take.confirm`,
`order.status`, `trade.fiat_sent`, `trade.release`, and `wallet.connection`.
The final exact identifier format is a mobile-team decision, but it must be
namespaced, stable, documented, and tested in Flutter widget tests.

### 17.3 Environment selection and relay discovery

The app's normal relay discovery may use bootstrap relays when it has no
operating relay. For Mortsom this is unsafe: a disconnected local relay must
produce a test failure, not public-network traffic. The test flavor must seed
the local relay before subscriptions begin and restrict discovery to the daemon
under test. A bootstrap fallback remains valid only in production builds.

## 17. Sequential implementation plan

Every phase has an exit gate. Do not start a later phase until the prior gate
passes; otherwise failures will be impossible to attribute.

### Phase 0 — repository and safety foundation

**Goal:** make Mortsom safe to develop without touching funds or the app.

1. Expand the empty Rust scaffold into a Cargo workspace. Select and document
   Rust crates for configuration, async process management, Docker control,
   WebDriver protocol access, serialization, redaction, and reporting before
   adding scenarios. Appium with UiAutomator2 is an external Android service;
   no TypeScript or Node application code belongs in Mortsom.
2. Create `mortsom.toml.example`, `.gitignore`, secret-redaction utilities,
   and config validation tests. Every `env:` secret reference must fail closed.
3. Implement `mortsom doctor` for Docker, Android SDK, emulator, adb, Appium,
   Java, Flutter, the mobile checkout, daemon reachability, relay port, and
   required environment variables. It must perform no payment and no order.
4. Implement run IDs, structured logs, artifact directories, and a global
   safety guard that refuses mainnet unless explicit opt-in is present.
5. Implement cap accounting interfaces before any wallet action. A missing cap
   is an error, never an unlimited default.

**Exit gate:** `mortsom doctor` produces a redacted report on a developer
machine and correctly reports each missing dependency or secret.

### Phase 1 — deterministic infrastructure

**Goal:** automatically start and prove the local test topology.

1. Add Docker Compose or an equivalent service module for `nostr-rs-relay`.
   Allocate a unique relay port and run data directory per run.
2. Implement relay readiness probing and explicit teardown. Add tests proving
   a failed startup does not remove another run's container.
3. Add external-local Mostro daemon readiness checks: pubkey, relay list, LND
   connection, and declared short timeouts. Do not launch or restart it yet.
4. Implement an event observer subscribed only to Mortsom actor identities and
   run-created order IDs. Store redacted ordered JSONL evidence.
5. Add a no-app smoke command that proves relay-to-daemon-to-relay event flow
   using a harmless metadata or readiness observation, not an order.

**Exit gate:** one command starts a local relay, verifies the local daemon uses
it, collects relay evidence, and always cleans up containers.

### Phase 2 — Android device farm of two

**Goal:** create reproducible Alice and Bob devices.

1. Implement AVD discovery/creation with pinned image, device profile, and
   snapshot. Keep the image/version matrix in configuration.
2. Implement independent Alice and Bob AVD paths and ports. Prove that killing
   Alice cannot affect Bob.
3. Implement boot/readiness, network host mapping, locale/time configuration,
   animation disabling, unlock, APK installation, and version verification.
4. Start Appium sessions and implement basic primitives: find-by-accessibility,
   tap, type, wait, screenshot, hierarchy dump, background, force-stop,
   relaunch, and clear-data.
5. Implement a smoke scenario that launches both copies of the app and records
   their initial screens without creating an identity or order.

**Exit gate:** `mortsom device smoke` brings up two clean devices, launches the
same specified APK, gathers evidence, and tears them down reliably.

### Phase 3 — mobile automation contract

**Goal:** make the classic app automatable without brittle selectors.

1. In `mobile`, add and document stable semantic identifiers for onboarding,
   navigation, settings, relay/node selection, NWC, order creation, and order
   state. Add widget tests that fail if each identifier disappears.
2. Add the Mortsom-only build configuration. It accepts local relay and daemon
   pubkey settings, disables production relay fallback, and visibly marks the
   environment. It must be impossible to enable accidentally in release builds.
3. Implement UI-driven onboarding for Alice and Bob with fresh generated test
   identities. Verify identities differ and survive app restart.
4. Implement the normal settings UI path for configuring a custom Mostro node
   and test relay. Verify the app reaches the local daemon's relay list.
5. Implement UI-driven NWC setup for both devices. Verify connected state and
   record only redacted wallet metadata.

**Exit gate:** both apps, using only UI actions, have distinct identities,
connected NWC wallets, the local relay, and the local daemon configured.

### Phase 4 — first real end-to-end trade

**Goal:** prove Mortsom closes the entire user journey once.

1. Implement screen objects and domain actions for creating a fixed-price sell
   order, discovering it, opening its detail screen, taking it, and checking
   visible statuses.
2. Register cancellation cleanup immediately after successful creation. Make
   cleanup idempotent and preserve its evidence.
3. Implement payment handling through the app's actual NWC flow; the runner
   may observe payment state but may not call NWC to advance a UI scenario.
4. Implement buyer invoice submission, fiat-sent confirmation, seller release,
   completion display, and payment-settlement correlation.
5. Implement a single ordered timeline artifact joining screenshots, UI states,
   relay events, wallet observations, and cleanup result.
6. Require explicit mainnet opt-in for this phase. Start at the smallest safe
   amount and abort before any payment exceeding caps.

**Exit gate:** `happy_sell` runs from clean devices to completed order, shows
the correct state on both apps, records matching relay/payment evidence, and
leaves no open order or unsettled payment.

### Phase 5 — recovery, failure, and cleanup

**Goal:** find the bugs users meet outside the happy path.

1. Add app-restart scenarios at each durable trade state. Assert restored state
   and absence of duplicate Nostr actions.
2. Add network loss/recovery, relay restart, app backgrounding, force-stop,
   slow event delivery, duplicate event, and stale event scenarios.
3. Add NWC cancellation, unavailable wallet, rejected invoice, expired invoice,
   failed payout, and retry-status scenarios using controlled wallet behavior
   where needed.
4. Add pending cancellation, expiration, cooperative cancellation, and
   unauthorized-action UX scenarios.
5. Add a final cleanup sweep and a `mortsom cleanup <run>` command. It must
   report unresolved resources honestly rather than pretending success.

**Exit gate:** a forced failure preserves diagnostics, executes cleanup once,
and a following clean run begins with no inherited app or order state.

### Phase 6 — CI and operational canary

**Goal:** make the suite useful continuously without turning it into noise.

1. Containerize runner dependencies or provide a pinned bootstrap installer for
   CI agents. Cache Android images and Flutter dependencies safely.
2. Run non-value-moving device and UI-contract tests on every pull request.
3. Run isolated integration smoke tests only in a secured CI environment with
   dedicated secrets and explicit budget policy.
4. Publish sanitized timeline, screenshots, logs, retry classification, and
   artifact links for failures.
5. Add scheduled mainnet canary with single-run locking, hard caps, alerting,
   manual review on failure, and accounting trend reports.
6. Add a runbook explaining infrastructure failures, app regressions, payment
   failures, relay failures, cleanup recovery, secret rotation, and incident
   response.

**Exit gate:** PR checks are deterministic and useful, while the canary can
detect a real integration regression without risking more than its configured
budget.

### Phase 7 — `mobilev2` adapter and parity

**Goal:** extend a proven harness to the new Flutter application without
duplicating its infrastructure or weakening the legacy suite.

1. Inventory `mobilev2` onboarding, node/relay configuration, NWC flow, order
   states, package/build process, and accessibility surface against the shared
   `AppAdapter` contract.
2. Add only the missing adapter capabilities or normalized states that are
   genuinely product-wide; do not leak v2 implementation types into the shared
   scenario kit.
3. Implement `mobile-v2` build, install, bootstrap, selector, and screen
   mapping modules. Keep all Rust-core interaction inside the v2 app; Mortsom
   still controls it exclusively through UI and public Android behavior.
4. Port `happy_sell` unchanged at the scenario layer. Differences belong in the
   adapter unless they expose a deliberate user-facing behavior difference.
5. Add contract tests for every v2 selector and state mapping, then add the
   cross-app parity suite for the common scenario set.
6. Run legacy and v2 in separate jobs with their own build/profile metadata so
   one application's failure cannot contaminate the other's artifacts.

**Exit gate:** the same normalized happy-path scenario runs against both app
profiles, produces comparable evidence, and reports intentional differences
explicitly rather than hiding them.
