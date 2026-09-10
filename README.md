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

Default layout:

```text
~/dev/
  mortsom/     this repository
  mobilev2/    checkout of MostroP2P/app with the automation contract
```

| For | Requirements |
|---|---|
| Harness | Rust stable (MSRV 1.89), Cargo, Git, Docker Engine and Compose with permission to use them |
| Source builds | Flutter on PATH, the app's `scripts/frb-generate.sh` and matching `flutter_rust_bridge_codegen` |
| Linux build | clang, CMake, Ninja, pkg-config, GTK 3 and libsecret >= 0.18.4 development files |
| Linux runtime | Xvfb, D-Bus, GNOME Keyring, AT-SPI bus and registry, ImageMagick `import` |
| Web build | The app's `scripts/build-web.sh`, wasm-pack, Python 3, rustup nightly with rust-src |
| Web runtime | Matching Chromium and ChromeDriver |

`doctor` reports what is missing for the selected platform; it installs nothing. Runs never open windows on
your desktop: Linux clients live on private virtual displays, Web clients in headless browser profiles.

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
source_dir = "../mobilev2"

[run]
amount_sats = 1000
max_order_sats = 5000        # per-order cap; required
max_run_spend_sats = 50000   # cumulative cap; required
state_timeout_secs = 90
settle_timeout_secs = 120
```

Caps are mandatory. Secrets are `env:` references resolved in memory; never paste macaroons, keys or
credentials into TOML. `*.local.toml`, `mortsom.toml`, `.env*` and `artifacts/` are gitignored. A config also
merges its optional `<name>.local.toml` overlay.

## Stack

```sh
./target/debug/mortsom stack up
./target/debug/mortsom stack status
./target/debug/mortsom stack down
```

The stack is the Compose project in [stack/](stack/): a regtest `bitcoind`, LND nodes for Alice, Bob and
Carol (the daemon's escrow node), an in-memory Nostr relay on `127.0.0.1:7000` and `mostrod`. Nothing else is
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
cancelled or a payment refunded. Both `up` and `down` refuse containers another directory created.

`stack logs [service]` shows a service's log by its name -- `mostro` (the default), `relay`, `bitcoind`,
`lnd-alice`, `lnd-bob` or `lnd-carol` -- through the same Compose project every other stack command uses,
so the container name never has to be known. `--follow` keeps streaming, `--since 10m` and `--tail 200`
bound it. The daemon logs at `info`, so a message it drops before dispatch (one that fails its structural
check, say) leaves no line; the run's `relay-events.jsonl` shows whether it reached the relay.

```sh
./target/debug/mortsom stack logs --follow          # mostrod, live
./target/debug/mortsom stack logs relay --since 5m
```

`--stack-dir` selects another directory holding the same files; the default is `stack`, so run the binary
from the repository root.

## Building the app

Without `--app-artifact`, `run` builds the selected target from `mobile.source_dir`. To build and verify
without launching clients:

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux
cargo run -p mortsom-coordinator --example prepare_client -- web
```

Build Linux and Web sequentially: bridge generation writes shared source files. Then reuse the outputs:

```sh
./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact ../mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform web --app-artifact ../mobilev2/build/web
```

For Linux, point at the executable inside the complete bundle. For Web, point at the complete output
directory. Mortsom hashes the whole bundle and records it. Rebuild after any app change or a changed daemon
identity.

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

The first four families are implemented; the others are roadmap ([spec.md](spec.md), sections 8 and 14).
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
run is refused while a journal is unresolved. Details and failure categories are in
[docs/runbook.md](docs/runbook.md).

## Commands

| Command | Purpose |
|---|---|
| `stack up`, `stack status`, `stack logs [service]`, `stack down` | Regtest environment lifecycle and service logs |
| `doctor --platform linux\|web` | Read-only prerequisite and readiness checks |
| `run <scenario>... [--tag <family>] [--platform linux\|web]` | Scenarios with isolated clients, caps and cleanup; one run per scenario |
| `cleanup <run-id>` | Recover a run's identities and orders |
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
