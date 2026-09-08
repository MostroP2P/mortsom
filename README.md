# Mortsom

End-to-end test harness for the Mostro app v2. It drives two real copies of the app, Alice and Bob, through a
complete Lightning trade against a local regtest stack, and proves the trade from the outside: what each
user saw, what the daemon published and what each wallet actually paid or received.

Clients run on **Linux** (default) or **Web**, both built from the same Flutter source. Android emulators are a
frozen, selectable option. [spec.md](spec.md) is normative; this file is the operator guide.

## Status

Accepted live cells: **Linux 0/2, Web 0/2**. See [docs/acceptance-ledger.md](docs/acceptance-ledger.md) for
attempts and [docs/HANDOFF.md](docs/HANDOFF.md) for the current plan.

**Transitional dependency.** The spec requires `mortsom stack up` to build the regtest stack on its own. The
current implementation still delegates to a sibling [Ortsom](https://github.com/MostroP2P/ortsom) checkout
at `../ortsom`, built in release or debug. Until the stack implementation lands, that checkout is required for
`stack up`, `stack status` and every run. It is the one item in this guide that contradicts the spec.

## Quick start

```sh
cargo build -p mortsom-coordinator --bin mortsom

./target/debug/mortsom stack up          # regtest chain, LND alice/bob/carol, relay, mostrod
./target/debug/mortsom stack status      # exits non-zero until usable

./target/debug/mortsom doctor --platform linux   # prerequisites only; no order, no payment

./target/debug/mortsom run happy_sell    # builds app v2, starts Alice and Bob, runs one trade
./target/debug/mortsom run happy_buy --platform web
```

Every `run` creates fresh Alice and Bob identities, runs one scenario, resolves every order it created and
removes its clients. Results land in `artifacts/<run-id>/`. The exit status is non-zero when the run fails.

## Requirements

Default layout:

```text
~/dev/
  mortsom/     this repository
  mobilev2/    checkout of MostroP2P/app with the automation contract
  ortsom/      transitional; see Status
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

`stack up` pulls the daemon image, builds or updates the stack and verifies the running container matches
the pulled digest. Do not run it before every scenario: it can regenerate the daemon identity, after which
both clients must be rebuilt. `stack down` destroys the environment and its credentials; it is never
evidence that an order was cancelled or a payment refunded.

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

## Running scenarios

| Scenario | Maker | Taker | Order of funding |
|---|---|---|---|
| `happy_sell` | Alice sells | Bob buys | Bob provides the payout invoice, then Alice pays the hold |
| `happy_buy` | Bob buys | Alice sells | Alice pays the hold first, then Bob provides the payout invoice |

Both continue with Bob marking fiat sent and Alice releasing. A pass requires both clients' completed state,
matching relay evidence, the hold settled once on Alice's node and the payout settled once on Bob's.

Tool overrides:

```sh
./target/debug/mortsom run happy_sell --xvfb /path/to/Xvfb
export MORTSOM_BROWSER=/path/to/chromium
export MORTSOM_CHROMEDRIVER=/path/to/chromedriver
```

Android: `--platform android` with an explicit legacy configuration. Instructions are archived in
[docs/history/legacy-android.md](docs/history/legacy-android.md).

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

Recovery reopens the same Alice and Bob identities from protected storage under
`$HOME/.local/state/mortsom/recovery/<run-id>`, cancels through ordinary UI controls and deletes the profiles
only after verified resolution. It never creates a new identity, retries a payment or releases escrow. A new
run is refused while a journal is unresolved. Details and failure categories are in
[docs/runbook.md](docs/runbook.md).

## Commands

| Command | Purpose |
|---|---|
| `stack up`, `stack status`, `stack down` | Regtest environment lifecycle |
| `doctor --platform linux\|web` | Read-only prerequisite and readiness checks |
| `run <scenario> [--platform linux\|web\|android]` | One scenario with isolated clients, caps and cleanup |
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
