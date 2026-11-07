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

The regtest stack is Mortsom's own: `mortsom stack up` builds it from nothing with Docker and Git (it fetches
and builds Mostro's `main`), and both Linux cells were re-run against it on 2026-09-09.

## Quick start

```sh
git clone https://github.com/MostroP2P/mortsom.git
cd mortsom
cargo build -p mortsom-coordinator --bin mortsom

./target/debug/mortsom stack up          # regtest chain, LND alice/bob/carol, relay, mostrod built from main
./target/debug/mortsom stack status      # exits non-zero until usable

./target/debug/mortsom doctor --platform linux   # prerequisites only; no order, no payment

./target/debug/mortsom run happy_sell    # builds app v2, starts Alice and Bob, runs one trade
./target/debug/mortsom run happy_buy --platform web
./target/debug/mortsom run --tag smoke   # the cheap subset: three scenarios, one run each
./target/debug/mortsom run               # the whole catalogue: every scenario, one run each
./target/debug/mortsom list              # scenarios by family; --details adds capabilities and deadlines
```

Every `run` creates fresh Alice and Bob identities, runs one scenario, resolves every order it created and
removes its clients. `--tag` runs a whole family the same way: one run per scenario, in sequence, with a
report at the end. `run` with neither a scenario name nor `--tag` runs every scenario `list` shows, in
alphabetical order, the same way; a scenario the platform cannot drive is skipped with its reason, never
failed. If a run is refused before it starts (no stack, a pending recovery journal), the refusal holds for the
rest of the batch, which is reported as not run. Results land in `artifacts/<run-id>/`. The exit status is
non-zero when a run fails.

The console report is a table and a verdict, nothing more:

```text
  ──────────────────────────────────────────────────────────────────────────────────────────────
  [PASS]  cancel_before_taken                                                             1m 32s
  [FAIL]  happy_sell                                                                      4m 12s
          automation-contract: `order.status` did not appear: element `order.status` not found
          within 30s
          run 20261014T193526Z-e7111efd3bc9
  [SKIP]  background_at_taken                                                                  -
          background_at_taken needs background, which Linux and Web clients and the stack's
          relay cannot cause
  ──────────────────────────────────────────────────────────────────────────────────────────────
  1 passed  1 failed  1 skipped  |  9m 05s
  lane      local
  artifacts artifacts
  ✗ 1 scenario failed
  failed: happy_sell
```

`FLAKY` marks a pass that needed a retry, and `STOP` a scenario a refused run kept from starting. While it
runs, the terminal shows one line per scenario started, slow steps such as client builds, and warnings; the
step-by-step narrative goes to each run's `run.log` and `timeline.md` (`RUST_LOG=mortsom=info` puts it back
on the terminal).

Colour follows each stream: a terminal gets it, a pipe or file does not. `NO_COLOR` turns it off everywhere,
`CLICOLOR_FORCE=1` or `FORCE_COLOR=1` keeps it on a pipe, and a terminal that does not advertise
`COLORTERM=truecolor` gets the nearest 256-colour shade.

## Requirements

A fresh installation needs only this repository. App source is fetched automatically into the ignored
`.mortsom/app/` directory; a sibling `mobilev2` checkout is optional.

| For | Requirements |
|---|---|
| Harness | Rust stable (MSRV 1.89), Cargo, Git, C/C++ linker, `protoc` 3.15 or later, Docker Engine and Compose with permission to use them |
| Source builds | Flutter/Dart on PATH, Bash and standard Unix tools, `flutter_rust_bridge_codegen` matching the app's pin, `cargo-expand` |
| Linux build | clang/libclang with `clang++` on PATH and, in the directory it resolves to, LLVM's linker (`ld.lld`, package `lld`) and archiver; CMake, Ninja, pkg-config, GTK 3 and libsecret >= 0.18.4 development files |
| Linux runtime | Xvfb, D-Bus, GNOME Keyring, AT-SPI bus and registry, ImageMagick `import` |
| Web build | wasm-pack, Python 3, rustup nightly with rust-src and the wasm32 target |
| Web runtime | Matching Chromium and ChromeDriver |

For Debian/Ubuntu, install the native build and Linux runtime dependencies:

```sh
sudo apt-get update
sudo apt-get install build-essential git curl unzip xz-utils zip protobuf-compiler \
  clang libclang-dev lld cmake ninja-build pkg-config libgtk-3-dev libsecret-1-dev \
  xvfb dbus gnome-keyring at-spi2-core imagemagick python3
```

Mortsom's LND client compiles its protobuf definitions at build time, and they use proto3 `optional`
fields, which need `protoc` 3.15 or later. Ubuntu and Pop!_OS 22.04 package 3.12, and the build then
stops in `fedimint-tonic-lnd` with `This file contains proto3 optional fields, but
--experimental_allow_proto3_optional was not set`. Install a release from
[protocolbuffers/protobuf](https://github.com/protocolbuffers/protobuf/releases) and put its `protoc`
first on `PATH`, or point `PROTOC` at it for the build:

```sh
PROTOC=/path/to/protoc/bin/protoc cargo build -p mortsom-coordinator --bin mortsom
```

`mortsom doctor` cannot check this: it is part of the harness that needs `protoc` to compile.

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
Carol (the daemon's escrow node), an ephemeral Nostr relay on `127.0.0.1:7500` and `mostrod`. Nothing else is
needed: the chain mines its own coins. The nodes publish 10501–10503, which, like the relay's 7500, stays
clear of a Polar network (10001–10003, relay 7000) and of Ortsom's regtest stack (7000, 10001–10004): both
harnesses can run on one machine at the same time with their default ports.

`stack up` builds `mostrod` from the current `main` of [MostroP2P/mostro](https://github.com/MostroP2P/mostro),
starts the chain, nodes and relay, funds the nodes, opens one balanced channel from each actor to Carol, mints a
daemon identity for this stack, starts the daemon as your user and waits for its info event on the relay. It
is re-runnable: funded wallets and open channels are left alone. Every `up` mints a new daemon identity, after
which both clients must be rebuilt, so do not run it before every scenario.

The daemon comes from exactly one of three sources, all recorded in `public.json`, the stack fingerprint,
`stack status` and run artifacts:

```sh
./target/debug/mortsom stack up                                   # Mostro's main, built here (default)
./target/debug/mortsom stack up --mostro-ref new-feature          # another Mostro branch...
./target/debug/mortsom stack up \
  --mostro-ref https://github.com/MostroP2P/mostro/pull/963       # ...or a pull request, built here
./target/debug/mortsom stack up --mostro-image mostrop2p/mostro:v0.19.3   # a published image, pulled as is
```

With no option the stack tests the daemon as it is about to ship, so app pull requests are reviewed against
Mostro's `main`. `--mostro-ref` names a branch or pull request instead: protocol changes, coordinated
app/daemon pull requests and compatibility testing. Either way `stack up` fetches the branch or the pull
request's head (fork contributions included) on every run into `.mortsom/mostro`, builds that exact commit
with Mostro's `docker/Dockerfile` as `mortsom/mostro:git-<commit>` and starts the daemon from it, so `main`
is rebuilt whenever it has moved. The first build compiles `mostrod` and takes several minutes; the layer
cache makes a repeated commit quick. A failed fetch or build stops `stack up`; it never falls back to an
older image. `--mostro-image` skips the build and pulls a published image, such as a release, pinned by its
registry digest. The two options are mutually exclusive.

`stack up` therefore needs Git and network access to GitHub as well as Docker; the daemon is fetched and
built before any service starts, so a failure there leaves the stack as it was. `main` moves: when it no
longer builds, or speaks a protocol newer than Mortsom's pinned `mostro-core`, `stack up` fails or the
scenarios do, and `--mostro-image mostrop2p/mostro:v0.19.3` brings back the release the acceptance ledger's
latest cells ran against. `stack status` shows the source in effect -- for a build, its branch or pull
request and commit -- for as long as the stack is up.

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
./target/debug/mortsom run happy_sell --ref main
./target/debug/mortsom run happy_buy --platform web --ref main
./target/debug/mortsom run happy_sell --ref https://github.com/MostroP2P/app/pull/410
./target/debug/mortsom run --tag happy-path --platform web \
  --ref https://github.com/MostroP2P/app/pull/410
# Replace the example with any branch that exists in MostroP2P/app:
./target/debug/mortsom run --tag cancellation --ref feature/my-fix
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

The captured bundle of a fetched build is kept in `.mortsom/app/bundles/`. The next invocation that
resolves the same commit, with the same app version, tool versions and build commands (daemon pubkey,
relay port, order expiry), re-verifies that bundle's fingerprint and uses it instead of building, which
skips several minutes of compilation. Its report then shows `app_build.origin = "cached"` with the
original build's facts. Anything that changes a key input (a new commit, a new stack identity, a toolchain
upgrade) builds again. The key does not see compiler flags set in the environment or upgraded system
libraries: after such a change, `--rebuild` builds even on a match and replaces the kept bundle. The four most
recently used bundles are kept. Local checkouts (`--source`) and `--artifact` are never kept or
reused.

The entire invocation, including a family selected with `--tag`, builds and captures **one bundle** and
uses it for every scenario. Reports preserve the requested selector, full ref and exact commit in
`app_build.remote_selection`, alongside before/after source state, commands, tool versions and bundle
fingerprint. A moving branch cannot change a family's code halfway through. Recovery uses the retained
captured bundle, without fetching again. Changing the daemon/stack still requires a new app build.

Builds run `flutter pub get`, `scripts/frb-generate.sh`, the Web WASM script when applicable, and Flutter's
build with the Mortsom entrypoint and isolated stack defines. A fetched revision that commits its generated
bindings skips `scripts/frb-generate.sh` and is built with them as committed; a local checkout always
regenerates them. Linux and Web builds must run sequentially.
The selected app code must provide `lib/main_mortsom.dart` and the automation contract needed by the
selected scenarios; an older or incompatible PR reports the missing file/control or build failure.

### Local source and prebuilt alternatives

```sh
./target/debug/mortsom run happy_sell --source ../mobilev2
./target/debug/mortsom doctor --platform web --source ../mobilev2
./target/debug/mortsom run happy_sell --platform linux \
  --artifact ../mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform web --artifact ../mobilev2/build/web
```

`mobile.source_dir` also preserves the explicit local-checkout workflow and its intentional local changes.
`mobile.app_ref` selects managed source instead; these config keys conflict. `--ref` or `--source`
overrides the configured source mode. `--artifact` bypasses fetching and building; the three are mutually
exclusive. Relative paths are resolved from the current directory. The same three flags select
mostro-cli's build under `--profile cli`: they always mean the client of the profile in use.

For Linux, point `--artifact` at the executable inside the complete bundle; for Web, at the output
directory. Mortsom hashes and captures the whole bundle. Prebuilt source history is explicitly unknown.
To compile and verify without launching clients, use the preparation example (optional branch/PR argument):

```sh
cargo run -p mortsom-coordinator --example prepare_client -- linux main
cargo run -p mortsom-coordinator --example prepare_client -- web https://github.com/MostroP2P/app/pull/410
```

These commands use the default config and the stack selected by `MORTSOM_STACK_DIR` (or `stack`). They
remove their temporary captured bundle on completion and create no order/payment.

## Testing mostro-cli (profile `cli`)

The same scenarios drive [mostro-cli](https://github.com/MostroP2P/mostro-cli) instead of the app with
`--profile cli` (or `mobile.profile = "cli"`). Each actor action is one `mostro-cli --output json`
subprocess with the actor's private `MCLI_DIR` and `HOME`; Mortsom pays and invoices from the stack's LND
nodes, since the CLI has no wallet. The contract is [docs/cli-contract.md](docs/cli-contract.md).

```sh
./target/debug/mortsom doctor --profile cli                       # git and cargo for a source build
./target/debug/mortsom run --profile cli --tag smoke              # builds main of MostroP2P/mostro-cli
./target/debug/mortsom run --profile cli happy_sell \
  --ref https://github.com/MostroP2P/mostro-cli/pull/123     # a branch or a pull request's head
./target/debug/mortsom run --profile cli --tag smoke --source ../mostro-cli      # local checkout
./target/debug/mortsom doctor --profile cli --artifact ../mostro-cli/target/release/mostro-cli
./target/debug/mortsom run --profile cli --tag smoke --artifact ../mostro-cli/target/release/mostro-cli
```

`--ref`, `--source` and `--artifact` are the app's flags read for mostro-cli: `--ref` must name a branch
or a pull request of `MostroP2P/mostro-cli`, and the three are mutually exclusive. `--platform` is refused
with the CLI. Source builds run `cargo build --release --locked --bin mostro-cli` with a shared target directory
under `.mortsom/mostro-cli/`. The run first reads the CLI's offline discovery (`mostro-cli schema`): a CLI
without the commands the profile needs is refused before an actor starts, and a scenario needing a
capability the CLI does not offer (rating without `order rate`, `pending_order_expires` without
`order new --expires-in`, `unauthorized_actions_at_taken`, which reads offered actions a CLI has none of)
is skipped with the reason. Each step keeps `cli.jsonl`, the sanitized transcript of the invocations it
made, instead of a screenshot.

The `mostro-cli e2e` workflow runs a scenario tag (smoke by default) by hand for any CLI branch or pull
request (`cli_ref` takes `https://github.com/MostroP2P/mostro-cli/pull/<number>`) and nightly against
`main`. It needs no secrets: the stack is regtest, built by the job. mostro-cli's own pull requests do
not run it yet: to test one you have reviewed, dispatch Mortsom's `mostro-cli e2e` workflow with the
pull request's URL as `cli_ref` (the job builds and runs that pull request's code).

`mortsom parity <mobile-v2 run> <cli run>` compares the two clients on shared scenarios. Accepted
differences go in `config/parity-intentional.toml`, scoped to the pair with
`profiles = ["mobile-v2", "cli"]`.

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

The first five families and the anti-abuse bonds are implemented; adversarial messages are roadmap
([spec.md](spec.md), sections 8 and 14). A scenario the app profile cannot support is skipped with a
reason, never failed and never counted. The bond family needs a stack whose daemon asks for bonds
(`stack up --bond`), and every other family one whose daemon does not: a run skips, with the reason, every
scenario written for the other kind.

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
declared floor is refused, by the app before sending it or, while it cannot judge yet, by the daemon; a
proper one the app calls `valid` is then accepted; no payment) and `payout_slots_capped_per_destination` (one more funded trade than the declared
per-destination cap, each paying out to a hold invoice Bob never settles; the daemon dispatches up to the
cap and holds the next payout back until Bob settles one, then every trade completes). The limits come
from `stack/limits.toml` (`payout_invoice_min_expiry_secs`, `max_inflight_payouts_per_destination`); the
app must carry the `invoice.error` readout and route the daemon's replacement request to the invoice form.

Anti-abuse bonds, on a stack built with `./target/debug/mortsom stack up --bond --replace`, whose daemon
asks both sides for a bond (`stack/mostro-bond.tpl.toml`: one percent of the order, never under 1000 sats,
slashed on a waiting timeout, half kept by the node). Every bond is paid from the actor's node and every
claim invoiced there, and each is sized against the policy the stack declares. Released:
`taker_bond_released_after_trade` (both bonds lock, the sell completes, both come back),
`seller_taker_bond_then_escrow` (a buy taken by Alice: her bond, then the escrow), `taker_bond_declined`
and `taker_bond_expires_unpaid` (Bob walks away from his bond, or lets it lapse; the order stays on the
book), `maker_bond_locked_before_publish` (Alice's order reaches the book only once her bond locks),
`maker_bond_abandoned` and `maker_bond_expires_unpaid` (her order never reaches it) and
`bonds_released_by_solver_ruling` (a ruling that slashes nobody). Slashed and claimed: the four
waiting-state timeouts, named after who failed the step (`buyer_taker_`, `seller_maker_`, `seller_taker_`
and `buyer_maker_bond_slashed_on_timeout`); a solver slashing either side (`taker_bond_slashed_by_solver`,
`maker_bond_slashed_by_solver`); and `bond_claim_fails_then_replaced`, a claim the daemon cannot pay,
asked for again. Nothing about a bond is published, so the proof is in the wallets: a bond returned fails
with no settled record, a slashed one settles once for exactly the bond, a claim settles once for exactly
the share. Switching back to the other families takes `stack up --replace`; each `stack up` mints a new
daemon identity, and so a new client build.

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
| `timings.json` | Where the run's time went: each phase (preflight, bring-up, each actor's bootstrap steps, scenario, failure attribution, discovery, relay quiet window, cleanup, payments, teardown) and, per actor, the count, total and longest time of each kind of UI request, and how many failed or were given up on. Diagnostics only |
| `run.log` | Redacted runtime diagnostics; `tail -f` it during a run |

A failed check fails its run even when the scenario passed; the report names the check under the scenario.

## Recovering a failed run

```sh
./target/debug/mortsom cleanup <run-id>
```

A submission the app never listed (a failure before or right after the create tap) is checked against the
daemon's order events on the relay for the run's window. The run itself performs that audit after the
scenario, even when the app is stuck on a screen from which its trade list cannot be read, and repeats it
after the configured quiet window: when the relay shows no order the journal does not already know across
that interval, the run records that the submission produced nothing and
releases its own profiles, so no cleanup is needed. A retained journal gets the same audit from cleanup,
taken again after the retained clients ran. When the relay does show
candidates, cleanup lists them with the exact commands: name the order with `--attribute-order alice=<order-id>`,
or state `alice=none` when the daemon created none. An id must have been published in the window and not be
known before the submission, and `none` is refused while any unknown order from that window is still open.

Recovery reopens the same Alice and Bob identities from protected storage under
`$HOME/.local/state/mortsom/recovery/<run-id>`, cancels through ordinary UI controls and deletes the profiles
only after verified resolution. It never creates a new identity, retries a payment or releases escrow. A new
run is refused by default while a journal is unresolved, and `stack up`/`stack down` are refused while a journal's stack
still exists. A journal whose Mortsom-owned regtest stack was destroyed can only be closed with
`recovery abandon <run-id> --reason stack-destroyed --confirm-run <run-id>`, which preserves evidence and
leaves the run failed: nothing is verified. Details and failure categories are in
[docs/runbook.md](docs/runbook.md).

Older journals without recorded environment facts cannot be abandoned. To keep such a journal pending
and intact while explicitly using a **different, verified Mortsom regtest stack**:

```sh
mortsom run --tag smoke --ref https://github.com/MostroP2P/app/pull/456 \
  --preserve-legacy-recovery <run-id>
```

Repeat the option for each pending legacy run, on every invocation. It authorizes only the new run;
historical orders, payments and network remain unverified. Both old actor runtimes must have stopped,
the current stack must be owned, both current wallets must report regtest, mainnet permission must be
disabled and the lane must not be canary. The new run writes `recovery-isolation.json` and a warning;
no old evidence or protected storage is changed. A new unresolved run blocks the following scenarios.

## Commands

| Command | Purpose |
|---|---|
| `stack up [--mostro-ref <branch-or-pr-url> \| --mostro-image <ref>] [--bond]`, `stack status`, `stack logs [service]`, `stack down` | Regtest environment lifecycle (mostrod from Mostro's `main` by default), daemon selection, the anti-abuse bond (`--bond`) and service logs |
| `doctor --platform linux\|web` | Read-only prerequisite and readiness checks |
| `run [<scenario>...] [--tag <family>] [--platform linux\|web]` | Scenarios with isolated clients, caps and cleanup; one run per scenario; every scenario when none is named |
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

The identifier contract test (`selectors_match_the_v2_app_contract`) checks every identifier the adapter
uses against the app's registry (`lib/core/automation/automation_ids.dart`) in the checkout
`MORTSOM_APP_CHECKOUT` names; a relative path is read from the workspace root. Unset, the test skips and
says so; set to a checkout without the registry, it fails. CI checks out the app's `main` for it.

```sh
MORTSOM_APP_CHECKOUT=../appv2 cargo test -p mortsom-adapter-mobile-v2 selectors_match_the_v2_app_contract
```

Everything in this repository is written in English. Read [AGENTS.md](AGENTS.md) and [spec.md](spec.md)
before contributing.

Releases are cut with `cargo release`, from `main` directly or from a
`release/vX.Y.Z` branch when the bump should be reviewed first; the changelog,
the tag and the GitHub release follow from it. See
[docs/releasing.md](docs/releasing.md).

## License

Mortsom is free software, licensed under the GNU Affero General Public License, version 3 or (at your
option) any later version (`AGPL-3.0-or-later`). See [LICENSE](LICENSE) for the full text.
