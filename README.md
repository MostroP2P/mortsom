<p align="center">
  <img src="./mortsom.jpg" alt="Mortsom logo" width="240">
</p>

# Mortsom

Mortsom is a Rust end-to-end harness for [Mostro app v2](https://github.com/MostroP2P/app).
It operates two independent clients through their public UI: **Alice is the seller and Bob is the buyer**.
It checks what both screens show, what the Nostr relay publishes, and what the dedicated Lightning wallets
actually paid. An order created by the harness is created through the app, never through a protocol shortcut.

**Linux is the default. Web is selectable. Android remains available as the legacy path, with no new Android
work in this checkpoint.** The shared regtest environment is managed by the sibling Ortsom CLI and survives
client runs.

- [Quick start](#quick-start)
- [Current implementation status](#current-implementation-status)
- [Requirements and project layout](#requirements-and-project-layout)
- [Configure Mortsom](#configure-mortsom)
- [Start, inspect and update the stack](#start-inspect-and-update-the-stack)
- [Check prerequisites with doctor](#check-prerequisites-with-doctor)
- [Build app v2 and reuse artifacts](#build-app-v2-and-reuse-artifacts)
- [Run scenarios on Linux or Web](#run-scenarios-on-linux-or-web)
- [Read results and inspect progress](#read-results-and-inspect-progress)
- [Recover a failed run](#recover-a-failed-run)
- [Troubleshooting](#troubleshooting)
- [Commands and development checks](#commands-and-development-checks)

## Quick start

Run these commands from the Mortsom repository. They assume the sibling checkouts and the tools described
below are installed. Use app and Ortsom revisions containing the coordinated automation/public-stack
contracts; the local working revisions are recorded in [the spec](spec.md#18-current-implementation-evidence).

```sh
cd ~/dev/mortsom

# Build the sibling CLI and this harness.
# Mortsom prefers an Ortsom release binary, so refresh that binary explicitly.
cargo build --manifest-path ../ortsom/Cargo.toml --release --bin ortsom
cargo build -p mortsom-coordinator --bin mortsom

# Create the persistent regtest stack and pull the current mostrod image.
./target/debug/mortsom stack up
./target/debug/mortsom stack status

# Check Linux tools, the app build pipeline and the running environment.
./target/debug/mortsom doctor --platform linux

# Build app v2, start independent Alice/Bob clients, and run one scenario.
./target/debug/mortsom run happy_sell
```

To select Web:

```sh
./target/debug/mortsom doctor --platform web
./target/debug/mortsom run happy_sell --platform web
```

To run the buy-order journey:

```sh
./target/debug/mortsom run happy_buy --platform linux
./target/debug/mortsom run happy_buy --platform web
```

**These are the supported invocations, not a claim of completed trade acceptance.** The current checkpoint
is exactly `happy_sell` and `happy_buy` on both platforms; all four live acceptance cells remain pending.
The implementation is being validated and the current blocker is described below. Run one scenario per
command. A desktop batch or tag selecting several scenarios is refused before clients start.

You do not need to start a relay, configure Alice/Bob keys, or launch Android emulators separately.
`stack up` prepares the environment; each scenario invocation prepares its own client identities.
When finished with the entire regtest environment, `./target/debug/mortsom stack down` explicitly destroys
its services and generated state. Resolve pending runs before doing that.

## Current implementation status

Status recorded on 2026-09-08; [spec.md](spec.md) is the detailed implementation plan and evidence ledger.

| Area | Verified state |
|---|---|
| Persistent stack | Ortsom delegation, public readiness/ownership checks and immutable mostrod image verification implemented |
| mostrod update | `stack up` pulls `mostrop2p/mostro:latest`; the observed development image is v0.18.7, not a permanent version pin |
| Linux/Web app preparation | Both source builds and complete bundle capture passed |
| Alice/Bob isolation | Fresh onboarding, restart and original-identity recovery through new handles passed on both platforms |
| UI drivers | Native AT-SPI and WebDriver input/readback, owned process shutdown and failure-path tests passed |
| Recovery | Private journals/profiles, original stack/bundle/origin checks, UI cleanup and tracked wallet verification implemented |
| Scenario narratives | `happy_sell` and `happy_buy` implemented, including their different invoice/funding order |
| Live trade acceptance | Linux **0/2**, Web **0/2**; no funded happy-path pass yet |
| Resource comparison | Deferred until the four-case review checkpoint is accepted |

The final pause verification passed **613 workspace tests**, formatting and strict clippy. Tests
cover the native tab/relay fixes, recovery evidence ordering and direct invoice routes. App v2 verification passed 338
Flutter tests and 412 Rust tests; the current armed Linux accessibility suite has five passing tests.
These checks establish implementation evidence, not successful live trades.

Three Linux `happy_sell` attempts are recorded in
[the live attempt ledger](spec.md#1814-live-checkpoint-attempt-ledger). The latest reached creation and take,
then timed out because the app opened the buyer-invoice form directly while the adapter expected an
order-detail status node. Source commits `e846715` (harness) and `0ed7efb` (app) correct that transition;
44 adapter tests and five focused app tests passed. Both client bundles must be rebuilt before the live
retry. All three attempts ended without invoices/payments and with verified cleanup; no reset was needed.

**Work is paused for the operator's remaining weekly allowance.** Read [the continuation record](docs/HANDOFF.md)
for exact repository revisions, environment state, completed checks and the next commands. Acceptance is
still Linux0/2 and Web0/2.

The other eighteen scenarios and CPU/RSS measurements are deferred. Android development is frozen.

## Requirements and project layout

The default checkout layout is:

```text
~/dev/
  mortsom/       Rust harness and reports
  ortsom/        Sibling CLI that owns the persistent regtest stack
  mobilev2/      Checkout of MostroP2P/app with the automation contract
```

An app directory named `app` is also fine: configure `mobile.source_dir` accordingly. Mortsom needs the
coordinated app contract in that checkout, not the deprecated `MostroP2P/mobile` app. No sibling runner or
source code is imported into Mortsom.

| Used for | Requirements |
|---|---|
| Harness | Rust stable compatible with the workspace MSRV (1.89), Cargo, Git, Docker Engine/Compose and permission to operate Docker |
| Source builds | Flutter on PATH, app-owned `scripts/frb-generate.sh`, its matching `flutter_rust_bridge_codegen`, shell tools checked by doctor |
| Linux build | clang, CMake, Ninja, pkg-config, GTK 3 development files, libsecret >=0.18.4 development files |
| Linux runtime | Xvfb, D-Bus, GNOME Keyring, AT-SPI bus/registry and ImageMagick's `import` screenshot command |
| Web build | App-owned `scripts/build-web.sh`, wasm-pack, Python 3, rustup nightly toolchain with rust-src |
| Web runtime | Compatible Chromium and ChromeDriver executables |

Doctor checks the actual selected path and reports missing tools; it does not install them. Follow the
app checkout's toolchain instructions for its Rust bridge version. Keep the matching browser and driver
available together. Source builds and cached Rust/Flutter outputs need substantially more disk space than
running an already built client.

Linux runs use separate virtual displays, accessibility buses and keyrings. Web runs use separate Chromium
profiles in headless mode and a run-owned loopback server. Current CLI runs do not open interactive client
windows on your desktop; inspect screenshots and UI artifacts as described below.

## Configure Mortsom

The default file is [`config/desktop.example.toml`](config/desktop.example.toml). For the default directory
layout, it already selects app v2, local regtest and the stack's two LND wallets.

For a private configuration:

```sh
cp config/desktop.example.toml mortsom.local.toml
# Edit paths and explicit caps in mortsom.local.toml.
./target/debug/mortsom --config mortsom.local.toml doctor --platform linux
./target/debug/mortsom --config mortsom.local.toml run happy_sell
```

Useful settings from the example:

```toml
[mobile]
profile = "mobile-v2"
source_dir = "../mobilev2"

[run]
lane = "local"
amount_sats = 1000
max_order_sats = 5000
max_run_spend_sats = 50000
jobs = 1
cleanup = true
allow_mainnet = false
infra_retries = 0
artifacts_dir = "./artifacts"
fiat_code = "ARS"
fiat_amount = 1000
payment_method = "cash"
state_timeout_secs = 90
settle_timeout_secs = 120
```

This is an excerpt, not a complete configuration: retain the wallet, relay, daemon and remaining sections
from the copied example. Amounts and caps are sats. The payment reservation accounts for the actual escrow
invoice plus the enforced routing-fee ceiling; the buyer's invoice can be lower than the advertised trade
amount because of daemon fees. Missing caps are errors. Desktop runs use one scenario and one job.

A config also merges its optional `<name>.local.toml` overlay. For example, `mortsom.toml` merges
`mortsom.local.toml`, and `config/desktop.example.toml` merges `config/desktop.example.local.toml`.
Paths in the examples assume the current working directory is the Mortsom repository.

The default `env:MORTSOM_*` references are resolved from the validated Ortsom connection descriptor in
memory. Do not paste generated macaroons, private keys or wallet credentials into TOML. Keep their protected
files or environment references. Private `*.local.toml`, `mortsom.toml`, `.env*` and artifacts are gitignored.
The desktop checkpoint requires both dedicated external LND wallets; desktop NWC recovery is deferred.

If Ortsom is elsewhere, pass the same checkout to every applicable command:

```sh
./target/debug/mortsom --ortsom-dir /absolute/path/to/ortsom stack status
./target/debug/mortsom --ortsom-dir /absolute/path/to/ortsom --config mortsom.local.toml run happy_sell
```

## Start, inspect and update the stack

```sh
./target/debug/mortsom stack up
./target/debug/mortsom stack status
```

`stack up` pulls the published mostrod `latest` image, resolves its immutable identity, delegates to
`ortsom stack up`, and verifies that the running container uses the selected image. The public status
includes the daemon identity, relay, wallet connection references and declared limits. Mortsom consumes
that public contract instead of reading private app or daemon databases.

Use `stack status` to inspect an existing environment. Do not call `stack up` before every scenario:
it is a setup/update operation and can regenerate the daemon identity. Client runs leave the shared stack
running. To update mostrod deliberately, first finish/recover outstanding runs, run `stack up`, verify
status and rebuild clients for the resulting identity.

An explicit clean reset is:

```sh
./target/debug/mortsom stack down
./target/debug/mortsom stack up
```

This destroys regtest state and generated credentials. It is not proof that an old order was canceled or a
payment refunded. Preserve failure evidence and resolve owned orders first. Do not reuse a client bundle
compiled for the previous daemon identity.

## Check prerequisites with doctor

```sh
./target/debug/mortsom doctor --platform linux
./target/debug/mortsom doctor --platform web
./target/debug/mortsom doctor --platform linux --json
./target/debug/mortsom doctor --platform linux --no-network
```

Doctor creates no order and makes no payment. Normal checks inspect host/build tools, stack identity and
readiness, the public relay/daemon, wallet network/capabilities and explicit configuration. `--no-network`
skips connection probes; it is not a substitute for validating the running stack before a trade. Desktop
configuration still depends on an existing valid stack descriptor.

With an existing artifact, doctor validates runtime prerequisites and the complete artifact instead of
requiring the source-build tools:

```sh
./target/debug/mortsom doctor --platform linux \
  --app-artifact ../mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom doctor --platform web --app-artifact ../mobilev2/build/web
```

Fix every failed prerequisite before a live run. An expected NWC setup skip during a run with external LND
wallets is not a skipped scenario; the app intentionally has no connected NWC wallet on that path.

## Build app v2 and reuse artifacts

Without `--app-artifact`, `mortsom run` builds the selected target from `mobile.source_dir`. It invokes the
app-owned bridge generation, the app's Web/WASM build when needed, and Flutter's Mortsom entry point with
the current test environment, daemon key and host relay. The app must visibly identify the test environment.

To build and validate without launching clients or creating an order/payment, use the development example:

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux
cargo run -p mortsom-coordinator --example prepare_client -- web
```

That example uses `config/desktop.example.toml` and `../ortsom`; it does not accept the CLI's `--config`
or `--ortsom-dir` options. It reports source/build provenance, verifies the captured bundle, removes its
temporary copy, and leaves Flutter's normal build outputs in the app checkout.

Reuse those outputs to avoid rebuilding on every diagnostic attempt:

```sh
./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact ../mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform web \
  --app-artifact ../mobilev2/build/web
```

For Linux, supply the executable inside the **complete bundle**, including its `lib/` and `data/`
neighbors. For Web, supply the complete output directory, including JavaScript, WASM and assets.
Mortsom captures the complete bundle and verifies its content hash before launch. A supplied artifact has
known content identity but explicitly unknown source/build history in the run report. Keep the source-build
log when tracing it back to a revision. Rebuild after runtime app changes or a changed stack identity.

## Run scenarios on Linux or Web

| Scenario | Maker | Taker | Funding order |
|---|---|---|---|
| `happy_sell` | Alice creates a sell | Bob buys | Bob provides the buyer invoice; Alice funds the seller hold |
| `happy_buy` | Bob creates a buy | Alice sells | Alice funds the seller hold first; Bob then provides the buyer invoice |

Both flows then have Bob mark fiat sent and Alice release. Passing requires both clients' final completed
state, matching relay evidence, the seller's settled hold and the buyer's settled payout. Escrow settlement
alone is a nonterminal payout-pending state. Exact amounts, fees and actual ledger records are checked;
repeated observations or multipath HTLC attempts are not counted as duplicate payments.

```sh
./target/debug/mortsom run happy_sell                     # Linux default
./target/debug/mortsom run happy_buy --platform linux
./target/debug/mortsom run happy_sell --platform web
./target/debug/mortsom run happy_buy --platform web
```

Each command creates fresh isolated Alice/Bob profiles, runs one narrative and performs cleanup. The
current desktop checkpoint refuses several selected scenarios in one invocation, including a tag that
expands to both happy paths. Desktop infrastructure failures are not automatically retried with the same
identities. Use `mortsom list` to inspect registration; the larger catalogue is not an acceptance claim.

Custom tool locations:

```sh
./target/debug/mortsom run happy_sell --xvfb /absolute/path/to/Xvfb
export MORTSOM_BROWSER=/absolute/path/to/chromium
export MORTSOM_CHROMEDRIVER=/absolute/path/to/chromedriver
./target/debug/mortsom run happy_sell --platform web
```

Doctor expects Xvfb on PATH; the run command additionally supports `--xvfb`. Recovery accepts
`MORTSOM_XVFB` when the original run used a custom Xvfb location.

Android is selected with `--platform android` and an explicit legacy configuration. The historical
setup, NWC/Polar examples, Appium and emulator instructions are in
[the legacy Android guide](docs/legacy-android.md). They are outside this checkpoint.

## Read results and inspect progress

A run prints its ID, checks, scenario outcome and artifact directory. Artifacts default to
`artifacts/<run-id>/`. The process exit status is nonzero when the run fails. A healthy setup table does
not override a failed scenario printed below it.

| Artifact | What it tells you |
|---|---|
| `summary.json` | Scenario result, failure category, checks, lane, versions and fingerprints |
| `environment.json` | Public stack/client identity and run context |
| `timeline.md`, `timeline.json` | UI steps, observed states, relay events and wallet assertions |
| `relay-events.jsonl` | Run-scoped public relay evidence |
| `wallet-payments.json` | Payment observations and budget totals |
| `cleanup.json` | Order cleanup, client shutdown and protected recovery retention |
| `recovery.json` | Original run ownership, public journal events and retention state |
| `alice/steps/`, `bob/steps/` | Screenshots and public UI trees captured at recorded steps |
| `run.log` | Runtime diagnostics |

For an active run, replace the example ID with the printed ID:

```sh
tail -f artifacts/<run-id>/run.log
```

Some phase checks are printed only after completion. The appearance of new step artifacts and activity in
the owned app/build processes help distinguish a bounded wait from a stopped process. Do not treat a quiet
log alone as proof of a hang or kill unrelated browser processes. UI/state/payment waits are bounded.

A pass requires the requested scenario to pass and cleanup/evidence checks to succeed. Skipped scenarios,
failed attempts, unfunded form probes and a regtest reset never count as a happy-path pass. See the
[parity ledger](docs/ortsom-ui-parity.md) and [spec attempt ledger](spec.md#1814-live-checkpoint-attempt-ledger)
for actual accepted results.

## Recover a failed run

Start by reading `summary.json`, the last UI artifacts, `cleanup.json` and `recovery.json`.
If the original run already completed cleanup, the closed journal records that fact. Otherwise:

```sh
./target/debug/mortsom cleanup <run-id>
# Include the same --config and --ortsom-dir if the originals were customized.
```

Protected profiles, the captured client bundle and a durable journal live under
`$HOME/.local/state/mortsom/recovery/<original-run-id>`, outside shareable artifacts. Recovery verifies the
original stack/bundle, reopens the same public Alice/Bob identities and the same Web origin, discovers
uncertain submissions and uses ordinary UI cancellation controls. It never creates a replacement identity,
retries a payment or releases escrow as a cleanup shortcut.

Tracked payment outcomes must agree with terminal UI state. Unused tracked buyer invoices can be canceled
through the dedicated LND wallet. Unknown outgoing payment state does not prove a refund or no spend;
uncertainty keeps recovery open. Profiles are deleted only after verified resolution and client shutdown.
An unresolved journal blocks a new desktop run even if a different artifact directory is selected.

There are two reporting cases:

- A journal with no business-action intents, or one already durably marked resolved, can finish inactive
  storage removal without reopening the UI or stack. This offline path prints its result and updates the
  original journal; it does not create a new scenario report.
- A normal UI recovery attempt writes a new report referring to the original run. It does not rewrite a
  failed scenario as a pass. Required recovery evidence is written before deletion. If final reporting
  fails after verified deletion, the closed durable manifest remains authoritative; an earlier public
  retained=true report can be conservative.

An abrupt kill leaves a durable active-runtime marker. Automatic reconciliation of orphaned client
processes is not implemented: first reconcile the owned processes before reusing the profiles. Do not
remove markers, reset the stack or delete the private directory just to bypass an unresolved-run refusal.
The [runbook](docs/runbook.md) has further failure and secret-rotation guidance.

## Troubleshooting

| Symptom | Action |
|---|---|
| Stack not configured or not ready | Use `stack status`; complete `stack up` before building/running clients |
| Unsupported/incomplete public stack status | Rebuild Ortsom with the public metadata contract; Mortsom prefers its release binary |
| Missing Flutter/bridge/native/Web tools | Run the selected platform's doctor and fix the named prerequisite |
| Browser session cannot start | Check executable overrides and browser/driver compatibility |
| App connects to an old daemon identity | Rebuild against the current stack; do not reuse the old artifact |
| New run refused by retained recovery | Inspect the original report and run `cleanup <original-run-id>` |
| Recovery reports changed stack or bundle | Preserve the original state/evidence and restore the matching environment or resolve it explicitly |
| `automation-contract` failure | Inspect the actor's last screenshot/UI tree; fix the app contract or adapter, then repeat with fresh actors |
| Healthy checks but failed scenario | Read the separate scenario outcome and cleanup result; infrastructure readiness is not trade acceptance |
| Cleanup/report write failure | Fix artifact permissions/disk space; inspect the durable recovery journal before assuming profiles remain |

## Commands and development checks

| Command | Purpose |
|---|---|
| `stack up`, `stack status`, `stack down` | Persistent regtest setup/update, inspection and explicit destruction |
| `doctor [--platform linux\|web]` | Read-only prerequisite and readiness checks |
| `list` | Registered scenario names, tags and requirements |
| `run <scenario> [--platform linux\|web\|android]` | One desktop scenario with isolated clients, observers, caps and cleanup |
| `cleanup <run-id>` | Original-identity desktop recovery or legacy resource cleanup |
| `publish <run-id> --out <dir>` | Build a sanitized publication bundle from existing artifacts |
| `accounting [--lane local\|ci\|canary]` | Summarize recorded outcomes and spend |
| `parity <run-a> <run-b>` | Compare existing profile run reports |
| `device`, `app`, `relay` | Existing legacy Android/app/relay operations |

Run `./target/debug/mortsom <command> --help` for exact options. `--config` and `--ortsom-dir` are global.
Mainnet use requires explicit confirmation and caps; the current desktop checkpoint operates only against
its verified local regtest environment.

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo llvm-cov --workspace --all-targets --fail-under-lines 80
```

The coverage requirement remains part of final delivery; it has not yet been claimed complete for this
checkpoint. App-owned Flutter/Rust tests live in the app repository. Independent reviews are recorded in
the spec. Existing Android CI workflows do not establish Linux/Web live scenario acceptance.

Everything committed here is written in English. Read [AGENTS.md](AGENTS.md), the normative
[spec](spec.md) and [crate rationale](docs/stack.md) before contributing.
