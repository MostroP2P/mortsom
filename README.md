# Mortsom

End-to-end test harness for the Mostro app v2. It drives two real copies of the app, Alice and Bob, through a
complete Lightning trade against a local regtest stack, and proves the trade from the outside: what each
user saw, what the daemon published and what each wallet actually paid or received.

Clients run on **Linux** (default) or **Web**, both built from the same Flutter source. [spec.md](spec.md) is
normative; this file is the operator guide.

## Status

Accepted live cells: **Linux 2/2, Web 2/2**. Web required app v2 changes (IndexedDB stores and store initialisation on the web) that are merged
upstream as MostroP2P/app#408; build the clients from the app's `main`. See [docs/acceptance-ledger.md](docs/acceptance-ledger.md) for every attempt and
[docs/HANDOFF.md](docs/HANDOFF.md) for the current plan.

The regtest stack is Mortsom's own: `mortsom stack up` builds it from nothing with Docker alone, and both
Linux cells were re-run against it on 2026-09-09.

## Quick start

```sh
git clone https://github.com/MostroP2P/mortsom.git
cd mortsom
cargo build -p mortsom-coordinator --bin mortsom

./target/debug/mortsom stack up          # regtest chain, LND alice/bob/carol, relay, mostrod
./target/debug/mortsom stack status      # exits non-zero until usable

./target/debug/mortsom doctor --platform linux   # prerequisites only; no order, no payment

./target/debug/mortsom run happy_sell    # builds app v2, starts Alice and Bob, runs one trade
./target/debug/mortsom run happy_buy --platform web
./target/debug/mortsom run --tag smoke   # the cheap subset: three scenarios, one run each
./target/debug/mortsom list              # scenarios by family; --details adds capabilities and deadlines
```

Every `run` creates fresh Alice and Bob identities, runs one scenario, resolves every order it created and
removes its clients. `--tag` runs a whole family the same way: one run per scenario, in sequence, with a
report at the end. Results land in `artifacts/<run-id>/`. The exit status is non-zero when a run fails.

## Requirements

A fresh installation needs only this repository. App source is fetched automatically into the ignored
`.mortsom/app/` directory; a sibling `mobilev2` checkout is optional.

| For | Requirements |
|---|---|
| Harness | Rust stable (MSRV 1.89), Cargo, Git, C/C++ linker, `protoc`, Docker Engine and Compose with permission to use them |
| Source builds | Flutter/Dart on PATH, Bash and standard Unix tools, `flutter_rust_bridge_codegen` matching the app's pin, `cargo-expand` |
| Linux build | clang/libclang, CMake, Ninja, pkg-config, GTK 3 and libsecret >= 0.18.4 development files |
| Linux runtime | Xvfb, D-Bus, GNOME Keyring, AT-SPI bus and registry, ImageMagick `import` |
| Web build | wasm-pack, Python 3, rustup nightly with rust-src and the wasm32 target |
| Web runtime | Matching Chromium and ChromeDriver |

For Debian/Ubuntu, install the native build and Linux runtime dependencies:

```sh
sudo apt-get update
sudo apt-get install build-essential git curl unzip xz-utils zip protobuf-compiler \
  clang libclang-dev cmake ninja-build pkg-config libgtk-3-dev libsecret-1-dev \
  xvfb dbus gnome-keyring at-spi2-core imagemagick python3
```

Install [Rust with rustup](https://www.rust-lang.org/tools/install),
[Flutter](https://docs.flutter.dev/install) and [Docker Engine with Compose](https://docs.docker.com/engine/install/)
using their supported instructions. Put Flutter's `bin` and Cargo's `bin` on `PATH`, then:

```sh
rustup update stable
cargo install cargo-expand --locked
# Match the app's pubspec.yaml pin. Current app builds use 2.11.1.
cargo install flutter_rust_bridge_codegen --version 2.11.1 --locked

# Additional Web build tools:
cargo install wasm-pack --locked
rustup toolchain install nightly --component rust-src --target wasm32-unknown-unknown
```

The supported app pipeline currently uses Flutter 3.41.9; the selected branch's `pubspec.yaml`,
`.github/workflows/ci.yml` and `.github/workflows/web-build.yml` own its tool versions. Mortsom runs
`scripts/frb-generate.sh --check` after fetching and reports the exact codegen install command if its pin
has changed. It installs no system tools. Install matching Chromium/ChromeDriver packages for Web runs;
`MORTSOM_BROWSER` and `MORTSOM_CHROMEDRIVER` can point to explicit executables.

`doctor` reports missing tools for the selected platform without creating orders or paying. For managed
source, it defers app-file and codegen-pin checks to `run`, so it works before the first checkout exists.
`--no-network` also skips stack, relay and wallet connections. Builds need Internet access for Git and
Dart/Rust dependencies, and several GiB of free disk space. Linux clients use private virtual displays;
Web clients use headless browser profiles.

## Configuration

The default file is [config/desktop.example.toml](config/desktop.example.toml). For a private copy:

```sh
cp config/desktop.example.toml mortsom.local.toml
./target/debug/mortsom --config mortsom.local.toml run happy_sell
```

Key settings:

```toml
[mobile]
profile = "mobile-v2"
# Both omitted: managed upstream main.
# app_ref = "main"           # branch or GitHub PR URL
# source_dir = "../mobilev2" # alternative: local developer checkout

[run]
amount_sats = 1000
max_order_sats = 5000        # per-order cap; required
max_run_spend_sats = 50000   # cumulative cap; required
state_timeout_secs = 90
settle_timeout_secs = 120
```

Caps are mandatory. Secrets are `env:` references resolved in memory; never paste macaroons, keys or
credentials into TOML. `*.local.toml`, `mortsom.toml`, `.env*`, `.mortsom/` and `artifacts/` are gitignored. A config also
merges its optional `<name>.local.toml` overlay.

## Stack

```sh
./target/debug/mortsom stack up
./target/debug/mortsom stack status
./target/debug/mortsom stack down
```

The stack is the Compose project in [stack/](stack/): a regtest `bitcoind`, LND nodes for Alice, Bob and
Carol (the daemon's escrow node), an ephemeral Nostr relay on `127.0.0.1:7000` and `mostrod`. Nothing else is
needed: the chain mines its own coins.

`stack up` pulls the pinned daemon release (`mostrop2p/mostro:v0.18.7`, the version the accepted cells ran
against), starts the chain, nodes and relay, funds the nodes, opens one balanced channel from each actor to
Carol, mints a daemon identity for this stack, starts the daemon as your user and waits for its info event on
the relay. It is re-runnable: funded wallets and open channels are left alone. Every `up` mints a new daemon
identity, after which both clients must be rebuilt, so do not run it before every scenario. Pass
`--mostro-image <reference>` to install another build; the reference is recorded in the stack fingerprint.

Generated state lives in the gitignored `stack/local/`: the actors' node credentials, the daemon's settings
(which hold its key, owner-only) and database, and `public.json`, the descriptor every command reads to
attach. `stack status` exits non-zero until the stack is usable, so it can be waited on. `stack down` stops
the services, removes their volumes and deletes `stack/local/`; it is never evidence that an order was
cancelled or a payment refunded. Both `up` and `down` refuse containers another directory created and name
that directory; `stack up --replace` removes such a stack and builds here instead, leaving the other
directory's generated state stale.

`stack logs [service]` shows a service's log by its name -- `mostro` (the default), `relay`, `bitcoind`,
`lnd-alice`, `lnd-bob` or `lnd-carol` -- through the same Compose project every other stack command uses,
so the container name never has to be known. `--follow` keeps streaming, `--since 10m` and `--tail 200`
bound it. The daemon logs at `info`, so a message it drops before dispatch (one that fails its structural
check, say) leaves no line; the run's `relay-events.jsonl` shows whether it reached the relay.

```sh
./target/debug/mortsom stack logs --follow          # mostrod, live
./target/debug/mortsom stack logs relay --since 5m
```

`--stack-dir` (or `MORTSOM_STACK_DIR`) selects another directory holding the same files; the default is
`stack`, so run the binary from the repository root. The published ports allow one stack per machine, so
checkouts and worktrees share it by pointing at the same directory:

```sh
export MORTSOM_STACK_DIR=~/dev/mortsom/stack
```

## Building the app

### Remote branch or pull request

With the default configuration, `run` fetches `main` from `MostroP2P/app`, builds it for the selected
platform and executes the scenarios. Use one selector flag for a branch or a pasted GitHub PR URL:

```sh
./target/debug/mortsom run happy_sell --app-ref main
./target/debug/mortsom run happy_buy --platform web --app-ref main
./target/debug/mortsom run happy_sell --app-ref https://github.com/MostroP2P/app/pull/410
./target/debug/mortsom run --tag happy-path --platform web \
  --app-ref https://github.com/MostroP2P/app/pull/410
# Replace the example with any branch that exists in MostroP2P/app:
./target/debug/mortsom run --tag cancellation --app-ref feature/my-fix
```

PR selectors fetch the **PR head commit**, including contributions from forks. They never test the base or
GitHub's synthetic merge. Only URLs for `https://github.com/MostroP2P/app/pull/<number>` are accepted
(an optional trailing slash is allowed); copy the PR's main page URL, without `/files`, query or fragment.
A branch selector names a branch in `MostroP2P/app`, not a tag or arbitrary Git expression.

Every invocation fetches again and resolves an exact commit. A missing ref or network/fetch failure stops
with an error; Mortsom never silently falls back to cached source. `.mortsom/app/repository.git` retains Git
objects to avoid downloading them again, while each build uses a new clean detached checkout under
`.mortsom/app/build-*/source`. Dart/Rust package caches are reused normally. Disposable checkouts and build
outputs are removed after bundle capture or failure; a hard-killed process can leave ignored build
directories. This directory contains no trader recovery profiles and must never be committed.

The entire invocation, including a family selected with `--tag`, builds and captures **one bundle** and
uses it for every scenario. Reports preserve the requested selector, full ref and exact commit in
`app_build.remote_selection`, alongside before/after source state, commands, tool versions and bundle
fingerprint. A moving branch cannot change a family's code halfway through. Recovery uses the retained
captured bundle, without fetching again. Changing the daemon/stack still requires a new app build.

Builds run `flutter pub get`, `scripts/frb-generate.sh`, the Web WASM script when applicable, and Flutter's
build with the Mortsom entrypoint and isolated stack defines. Linux and Web builds must run sequentially.
The selected app code must provide `lib/main_mortsom.dart` and the automation contract needed by the
selected scenarios; an older or incompatible PR reports the missing file/control or build failure.

### Local source and prebuilt alternatives

```sh
./target/debug/mortsom run happy_sell --app-source ../mobilev2
./target/debug/mortsom doctor --platform web --app-source ../mobilev2
./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact ../mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform web --app-artifact ../mobilev2/build/web
```

`mobile.source_dir` also preserves the explicit local-checkout workflow and its intentional local changes.
`mobile.app_ref` selects managed source instead; these config keys conflict. CLI `--app-ref` or
`--app-source` overrides the configured source mode. `--app-artifact` bypasses fetching and building;
the three CLI choices are mutually exclusive. Relative paths are resolved from the current directory.

For Linux, point `--app-artifact` at the executable inside the complete bundle; for Web, at the output
directory. Mortsom hashes and captures the whole bundle. Prebuilt source history is explicitly unknown.
To compile and verify without launching clients, use the preparation example (optional branch/PR argument):

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux main
cargo run -p mortsom-coordinator --example prepare_client -- web https://github.com/MostroP2P/app/pull/410
```

These commands use the default config and the stack selected by `MORTSOM_STACK_DIR` (or `stack`). They
remove their temporary captured bundle on completion and create no order/payment.

## What it tests

Scenarios are grouped in families. Each scenario carries its family name as a tag; `smoke` cuts across
families to mark the cheap subset (`happy_sell`, `happy_buy`, `cancel_before_taken`).

| Family | Tag | What it covers |
|---|---|---|
| Happy path | `happy-path` | Trades that work: sell, buy, range, market price, rating |
| Cancellations | `cancellation` | Orders ended before the sats move — unilateral, abandoned, cooperative |
| Expirations | `expiration` | The daemon's own clocks reaping what nobody touched |
| Disputes | `dispute` | Disputes opened, resolved by a solver, or undone by the parties themselves |
| Payment failures | `payment-failure` | Invoices the daemon cannot pay, how it recovers, and how many unresolved payouts it lets one payee pin |
| Anti-abuse bonds | `bond` | Bonds released, slashed and claimed |
| Adversarial | `adversarial` | Malformed, replayed, out-of-state and unauthorised messages |

The first five families are implemented; the others are roadmap ([spec.md](spec.md), sections 8 and 14).
A scenario the app profile cannot support is skipped with a reason, never failed and never counted.

## Running scenarios

| Scenario | Maker | Taker | Order of funding |
|---|---|---|---|
| `happy_sell` | Alice sells | Bob buys | Bob provides the payout invoice, then Alice pays the hold |
| `happy_buy` | Bob buys | Alice sells | Alice pays the hold first, then Bob provides the payout invoice |
| `range_order` | Alice sells a fiat range | Bob buys at an amount inside it | As `happy_sell`; the daemon publishes the remainder as a new order of Alice's, which she cancels |
| `range_buy_order` | Bob buys a fiat range | Alice sells at an amount inside it | As `happy_buy`; the remainder becomes a new order of Bob's, which he cancels |
| `market_price_order` | Alice sells at market price | Bob buys | As `happy_sell`; the daemon prices the take (its exchange-rate source must be reachable) |
| `full_trade_with_rating` | Alice sells | Bob buys | As `happy_sell`, then each rates the other; the daemon publishes each rating |

Trades continue with Bob marking fiat sent and Alice releasing. A pass requires both clients' completed
state, matching relay evidence, the hold settled once on Alice's node and the payout settled once on Bob's.

Cancellations, all with Alice as maker and Bob as taker: `cancel_before_taken` (Alice cancels a pending
order), `cancel_after_take_unpaid` (Bob takes and walks away; the daemon reaps the take after its window),
`taker_cancel_at_taken` (Bob cancels before Alice pays), `cooperative_cancel` (both agree to undo a funded
order; the escrow returns) and `unilateral_cancel_rejected` (Alice alone cannot cancel a funded order; the
relay never shows it cancelled during the window). The maker's own screen never proves a cancellation: the
relay must show it.

Expirations, each waiting for one of the daemon's clocks (`stack/limits.toml`): `pending_order_expires`
(nobody takes the order; the test build asks the daemon for the declared `order_expiration_secs`),
`hold_invoice_expires` (the seller never pays) and `buyer_invoice_deadline` (the buyer of a funded order
never invoices; the escrow comes back). The daemon has no fiat-payment window, so none is driven.

Disputes, each on a funded order with Alice as seller and Bob as buyer: `dispute_settled_by_solver` (Bob
paid, Alice will not release; Bob disputes and the solver settles for him), `dispute_canceled_by_solver`
(Bob never paid; Alice disputes and the solver cancels for her, returning the escrow) and
`dispute_released_by_seller` (Bob disputes, Alice releases anyway; the daemon closes the dispute). The
solver is registered per run with the daemon's own key from the stack's owner-only settings, so the two
solver scenarios run only against a Mortsom-owned stack and are skipped elsewhere. The proof is the daemon's
published dispute event after each step, never a screen.

Payment failures, all with Alice as seller and Bob as buyer and Bob's invoices issued on the node Mortsom
holds for him: `payout_fails_then_replaced` (Bob withdraws his payout invoice before Alice releases; the
daemon settles the escrow, fails to pay, retries as it declares and asks for a new invoice, which Bob hands
in; only the replacement settles), `payout_invoice_below_expiry_floor` (an invoice shorter than the
declared floor is refused with the reason kept on screen as `invoice.error`; a proper one is then accepted;
no payment) and `payout_slots_capped_per_destination` (one more funded trade than the declared
per-destination cap, each paying out to a hold invoice Bob never settles; the daemon dispatches up to the
cap and holds the next payout back until Bob settles one, then every trade completes). The limits come
from `stack/limits.toml` (`payout_invoice_min_expiry_secs`, `max_inflight_payouts_per_destination`); the
app must carry the `invoice.error` readout and route the daemon's replacement request to the invoice form.

Range and rating scenarios need an app build carrying the range and star identifiers
(`order.create.range`, `order.create.fiat_min`, `order.create.fiat_max`, `trade.rate.star.<n>`); an older
build fails them as a contract gap.

Web finds Chromium and ChromeDriver on `PATH`. When `PATH` offers the Chromium snap wrapper, which runs
confined and cannot write browser profiles under `~/.local`, Mortsom uses the unconfined `chrome` and
`chromedriver` inside the snap instead; `mortsom doctor --platform web` shows what was chosen. Tool overrides:

```sh
./target/debug/mortsom run happy_sell --xvfb /path/to/Xvfb
export MORTSOM_BROWSER=/path/to/chromium
export MORTSOM_CHROMEDRIVER=/path/to/chromedriver
export MORTSOM_WEBDRIVER_VERBOSE=1   # keep ChromeDriver's full log (DevTools events) as <actor>/driver.log
```

## Reading results

| Artifact | Contents |
|---|---|
| `summary.json` | Outcome, failure category, checks, versions, fingerprints |
| `timeline.md` | UI steps, observed states, relay events, wallet assertions |
| `alice/steps/`, `bob/steps/` | Screenshots and accessibility trees per step |
| `relay-events.jsonl` | Public relay evidence scoped to the run |
| `wallet-payments.json` | Payment observations and budget totals |
| `cleanup.json`, `recovery.json` | Order resolution, client shutdown, retained storage |
| `environment.json` | Stack and client identity |
| `run.log` | Redacted runtime diagnostics; `tail -f` it during a run |

A healthy checks table above a failed scenario is still a failed run.

## Recovering a failed run

```sh
./target/debug/mortsom cleanup <run-id>
```

When the app never listed the order a submission created, name it with `--attribute-order alice=<order-id>`,
or state `alice=none` when the daemon created none. Both are checked against the daemon's order events on the
relay for the run's window: an id must have been published then and not be known before the submission, and
`none` is refused while any unknown order from that window is still open.

Recovery reopens the same Alice and Bob identities from protected storage under
`$HOME/.local/state/mortsom/recovery/<run-id>`, cancels through ordinary UI controls and deletes the profiles
only after verified resolution. It never creates a new identity, retries a payment or releases escrow. A new
run is refused while a journal is unresolved, and `stack up`/`stack down` are refused while a journal's stack
still exists. A journal whose Mortsom-owned regtest stack was destroyed can only be closed with
`recovery abandon <run-id> --reason stack-destroyed --confirm-run <run-id>`, which preserves evidence and
leaves the run failed: nothing is verified. Details and failure categories are in
[docs/runbook.md](docs/runbook.md).

## Commands

| Command | Purpose |
|---|---|
| `stack up`, `stack status`, `stack logs [service]`, `stack down` | Regtest environment lifecycle and service logs |
| `doctor --platform linux\|web` | Read-only prerequisite and readiness checks |
| `run <scenario>... [--tag <family>] [--platform linux\|web]` | Scenarios with isolated clients, caps and cleanup; one run per scenario |
| `cleanup <run-id>` | Recover a run's identities and orders |
| `recovery abandon <run-id> --reason stack-destroyed --confirm-run <run-id>` | Close a journal whose regtest stack is provably destroyed; the run stays failed |
| `list` | Registered scenarios |
| `publish <run-id> --out <dir>` | Redacted evidence bundle |
| `accounting`, `parity` | Spend trends and run comparison |

## Development

```sh
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --all-targets
cargo llvm-cov --workspace --all-targets --fail-under-lines 80
```

Everything in this repository is written in English. Read [AGENTS.md](AGENTS.md) and [spec.md](spec.md)
before contributing.

Releases are cut with `cargo release` from a `release/vX.Y.Z` branch; the
changelog, the pull request and the GitHub release follow from it. See
[docs/releasing.md](docs/releasing.md).
