# Mortsom specification

**Version:** 1.0

**Updated:** 2026-09-16

**Status:** normative for all new work

**Target clients:** Mostro app v2 (`MostroP2P/app`); mostro-cli (`MostroP2P/mostro-cli`, profile `cli`)

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

Mortsom therefore targets app v2 in two execution modes, **Linux** and **Web**, both produced from the same
Flutter source. The Android emulator path has been removed: no emulator, Appium, adb or APK code remains in
the repository, and `--platform` accepts only `linux` and `web`.

The same scenarios also drive **mostro-cli**, the command-line client, as the application profile `cli`
(section 6.3). One run then tests the daemon and the CLI together, and pointing it at a mostro-cli pull
request makes it that repository's end-to-end check. The CLI is driven only through its machine interface
(`--output json`), always as a subprocess.

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
| Alice | Client user (app v2 or mostro-cli). Seller in `happy_sell`, taker-seller in `happy_buy` | LND `alice` |
| Bob | Client user (app v2 or mostro-cli). Buyer in `happy_sell`, maker-buyer in `happy_buy` | LND `bob` |
| Carol | The server side: the LND node behind `mostrod` | LND `carol` |

Alice and Bob each run their own isolated client: a copy of app v2, or for profile `cli` mostro-cli with a
private identity store (section 6.3). Their wallets are external LND nodes in the stack, operated by Mortsom
in the role of the user's wallet: it pays the invoice the client shows and produces the invoice the user
gives the client. Wallet actions never publish protocol messages.

Both actors run on the same platform in one invocation. Mixed pairs are out of scope.

## 4. Operator workflow

```sh
# Regtest environment owned by Mortsom.
mortsom stack up
mortsom stack status
mortsom stack down

# The same stack with the daemon's anti-abuse bond switched on (section 8.8).
mortsom stack up --bond --replace
mortsom run --tag bond

# Prerequisites for a platform. No order, no payment.
mortsom doctor --platform linux
mortsom doctor --platform web

# Scenarios. Linux is the default platform.
mortsom run happy_sell
mortsom run happy_sell --platform web
mortsom run happy_buy --platform linux
mortsom run happy_buy --platform web

# Select a remote branch or paste a GitHub PR URL; use its exact head.
mortsom run happy_sell --ref main
mortsom run --tag happy-path --platform web --ref https://github.com/MostroP2P/app/pull/123

# The whole catalogue: every registered scenario, one run each.
mortsom run

# Explicit developer checkout (including local changes).
mortsom run happy_sell --source ../mobilev2

# Build from source even when an identical kept bundle exists.
mortsom run happy_sell --rebuild

# Reuse a build instead of compiling from source.
mortsom run happy_sell --platform linux --artifact /path/to/bundle/mostro
mortsom run happy_sell --platform web   --artifact /path/to/build/web

# mostro-cli instead of the app (profile `cli`, section 6.3): main of MostroP2P/mostro-cli by default,
# a branch or pull request, a local checkout, or a prebuilt executable.
mortsom doctor --profile cli
mortsom run --profile cli --tag smoke
mortsom run --profile cli happy_sell --ref https://github.com/MostroP2P/mostro-cli/pull/123
mortsom run --profile cli happy_buy --source ../mostro-cli
mortsom run --profile cli --tag smoke --artifact /path/to/mostro-cli

# Recover a run that left orders, payments or profiles behind.
mortsom cleanup <run-id>
```

`run` attaches to an already ready stack and fails with instructions when it is absent. It never creates or
destroys the stack. One invocation runs the selected scenarios sequentially, each with fresh actors. Unknown platforms, `android`
included, fail before anything is built or launched.

## 5. Stack

Mortsom owns its regtest environment and needs no other harness to create it. `mortsom stack up` builds the
whole world from nothing on a machine with Docker, and Git with access to GitHub to fetch the daemon's source
(Docker alone with `--mostro-image`): a regtest chain, three Lightning nodes with balanced channels, a Nostr
relay and a `mostrod` with an identity minted for this stack. `stack status` reports it and
`stack down` removes it, volumes included.

| Service | Role | Host port |
|---|---|---|
| `bitcoind` | Regtest chain; mines its own coins, so nothing needs external funding | none |
| `lnd-alice` | Alice's wallet | 10501 |
| `lnd-bob` | Bob's wallet | 10502 |
| `lnd-carol` | The daemon's escrow node | 10503 |
| `relay` | Nostr relay reachable only from this host; its SQLite database is an unmounted file in the container, so it lives and dies with the stack | 7500 |
| `mostro` | The daemon under test | none |

Topology: Alice and Bob each open one channel to Carol, funded at 1,000,000 sats with 500,000 pushed across,
so both have outbound liquidity to pay a hold invoice and inbound liquidity to receive a payout. There is
deliberately no Alice-to-Bob channel: every payment has the daemon's node on one end, and a second route would
only let a payment succeed by accident. Carol exists so the daemon never escrows on a node an actor pays from.

`stack up` must:

1. Resolve the daemon image from exactly one source:
   - no option: the `main` branch of `MostroP2P/mostro`, built as a ref (below), so ordinary app review runs
     against the daemon as it is about to ship;
   - `--mostro-ref <branch-or-pr-url>`: another branch of `MostroP2P/mostro` or a canonical
     `https://github.com/MostroP2P/mostro/pull/<number>` URL, for protocol changes, coordinated app/daemon
     pull requests and explicit compatibility testing;
   - `--mostro-image <reference>`: a published image built elsewhere, such as a release.

   The two options are mutually exclusive.

   A pulled image must carry exactly one official repository digest; a failed pull is an error, never
   permission to use a stale cached image. A ref is fetched on every invocation into an owned bare cache
   under `.mortsom/mostro` (branches from `refs/heads/<branch>`, pull requests from `refs/pull/<n>/head`,
   so fork contributions resolve to the contributor's head), resolved to a full commit, checked out
   detached and clean in a disposable directory and built with the repository's `docker/Dockerfile` as
   `mortsom/mostro:git-<commit>`, labelled with that commit. The build runs every time, relying on the
   layer cache for repeated commits; a failed fetch or build is an error and never starts an older image.
   A built image has no registry digest; its identity is the inspected image ID plus the Git provenance.
2. Start the chain, nodes and relay; wait for each to be usable; fund the nodes and open the channels above,
   skipping work already done so the command is re-runnable against a half-built stack.
3. Mint the daemon identity for this stack, write its settings with the declared test limits, and start the
   daemon as the invoking user so it can read owner-only files. Verify the running container is the
   prepared image ID. With `--bond` the settings also carry the committed `[anti_abuse_bond]` fragment
   (`stack/mostro-bond.tpl.toml`): the daemon then asks every maker for a bond before it publishes the order
   and every taker before the trade starts, and the descriptor declares that policy (side, share of the
   order, floor, timeout slashing, the node's share of a slash, claim window) to every run on the stack. Such
   a stack also declares a longer untaken-order lifetime (`bond_order_expiration_secs` in `stack/limits.toml`)
   in place of `order_expiration_secs`: a timeout slash returns the order to `pending` with the expiry it had
   at creation, and the claim that follows is paid before the maker cancels it. A stack built without it
   declares no bond. The base template never carries the section itself, so no
   other family is ever run against a bonded daemon by accident.
4. Wait until the daemon publishes its info event on the relay, then validate regtest networks, advertised
   relay endpoints, channel liquidity and declared limits. The info event's `bond_*` tags must advertise
   exactly the bond the stack declares, or, without `--bond`, no bond: the settings file is what the daemon
   was told, its info event what it says it does.
5. Write the public connection references, daemon public identity, image identity (image ID, container
   creation reference and repository digest when one exists), daemon source provenance (kind `image` or
   `git-ref`; the requested selector; for a ref, the repository, fetched ref and full commit; `pinned-image`
   is read from records written while a pinned release was the default, and never written) and stack
   fingerprint to Mortsom's ignored local overlay (descriptor schema 2). Run artifacts carry the same
   provenance, so a report names the exact app and daemon commits tested together. Node credentials and the
   daemon's private key stay in owner-only files that are never echoed, logged or copied into artifacts.

`stack status` exits non-zero until the stack is usable, so it can be waited on. It reports each service,
each node's active channels, the daemon identity, image and source in effect, and flags a running daemon
container that is not the recorded image. A stopped container reads as down with its state; only one that
no longer exists reads as gone.

`stack down` stops the services, removes their volumes and deletes the generated identity and credentials,
so no public key is left that nothing answers on. Both `up` and `down` refuse to touch containers or volumes
that this stack directory did not create, and name the directory that did. The one exception is
`stack up --replace`, an explicit operator action: it removes the `mortsom-regtest` project by name, whatever
created it, before building, and the replaced directory's generated state is stale from then on. The
published ports allow one stack per host, so checkouts share it by selecting the same stack directory
(`--stack-dir`, or `MORTSOM_STACK_DIR`).

`run` never creates or destroys the stack, and changes it in one way only: on a Mortsom-owned stack it keeps
the regtest chain tip fresh. `stack up` mines only while it builds. About two hours after the newest block the
LND nodes report `synced_to_chain: false`, and mostrod then enters maintenance mode: it rejects new orders
and takes, and skips bond payout cycles and failed-payment retries. It leaves maintenance mode only once LND
has been synced for five minutes. So before each run's preflight (one per scenario, and `cleanup`), when
the newest block is more than an hour old, the run mines one block to the miner wallet; one block barely
moves a CLTV. It mines only when attached to a stack this checkout owns and when bitcoind itself reports
`regtest`. The run's `chain-tip` check records the tip's age and any block mined; a tip that cannot be read
or refreshed fails it, and the preflight with it. If, having mined, the daemon advertises maintenance mode,
the run waits up to seven minutes for it to leave before the preflight judges the stack.
Scenario teardown never removes containers or volumes. A
stack reset is an explicit operator action; it is not evidence that an order was cancelled or a payment
refunded. The published ports (7500, 10501–10503) are chosen not to collide with a local Polar network
(10001–10003), a relay on 7000, or Ortsom's regtest stack (7000, 10001–10004), so Mortsom runs beside any of
them on one host without configuration.

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

The app is built with `flutter build web --no-web-resources-cdn`, including its Rust/WASM bundle, and driven
through W3C WebDriver. The bundle carries its own CanvasKit, so the engine loads from the run's loopback origin,
never from a CDN.

Mortsom serves the bundle on loopback with `Cross-Origin-Opener-Policy: same-origin` and
`Cross-Origin-Embedder-Policy: require-corp`, limits requests to the bundle root, and owns every request task.
Alice and Bob use separate browser profiles with the sandbox enabled. Elements are located by exact
`flt-semantics-identifier` values with escaped selectors; only public DOM and accessibility interactions are
allowed. No JavaScript calls into Dart, Rust or application state.

Flutter may rebuild offscreen semantics after focus or scroll: re-find elements after transitions, scroll
through user actions, and wait for interactability with bounded predicates instead of fixed sleeps.

### 6.3 CLI (profile `cli`)

`--profile cli` (or `mobile.profile = "cli"`) drives mostro-cli instead of the app; `--platform` picks an
app platform and is refused with it. Each actor action is one child process of the CLI's machine
interface (`--output json`, contract version 1): one versioned outcome document per invocation, whose
status (`ok`, `error`, `uncertain`) and stable error code decide the result; prose is never parsed. The
contract Mortsom relies on is [docs/cli-contract.md](docs/cli-contract.md). The CLI is always a subprocess,
never a linked library: Mortsom proves the trade from the outside (section 1), so it tests the executable
mostro-cli ships, its argument parsing, outcome documents, exit codes and identity store included, and each
run picks the build under test (section 7) instead of fixing it when Mortsom is compiled.

Each actor owns a private `MCLI_DIR` and `HOME` (the CLI's identity store), retained under the recovery
journal like an app profile. The child inherits no other environment, runs in the actor's private home,
and reaches only the daemon and the run relay the build names (`--mostro-pubkey`, `--relays`,
`--transport nip44`). Invocations of one actor never overlap. Invoices travel on standard input
(`--invoice -`), never in process arguments. The lifecycle maps onto the store: first launch is
`identity init` on an empty store, a relaunch reads `identity show`, and a recovery reopens the retained
store. Observed state is the daemon's report to the order's party (`order info`, a read-only query);
requests that arrive while the actor is idle (a hold invoice for a maker, a payout request after the
seller paid, a bond, a slashed bond's share) are read with `message list`. For a CLI actor the outcome
documents stand in for the screen of section 2: its normal UI is the machine interface, and its visible
order ID and state are the ones they report.

The CLI has no wallet, so both actors are always on the external-wallet path: Mortsom pays the hold
invoice and bonds from the nodes it holds and hands the CLI the invoices it issued there. It offers every
command at any time and cannot be told to expire an order sooner unless its `order new` takes
`--expires-in`; what the built CLI offers is read from its offline discovery (`mostro-cli schema`), and a
capability whose commands it lacks is not claimed, so its scenarios are skipped with the reason. A CLI
without the commands the profile cannot run without (identity, the order lifecycle, `message list`)
fails before any actor starts.

## 7. Client builds and automation contract

`--artifact`, `--source` and `--ref` select the build of the client the profile drives: the app for
`mobile-v2`, mostro-cli for `cli`. The profile is `--profile`, else `mobile.profile` (local overlay
included). A known `--profile` replaces `mobile.profile` before the configuration is resolved and
validated, so it also stands in for a reference to an unset variable or an unknown configured profile; an
unknown one replaces nothing and is refused. The three conflict, and a `--ref` outside the profile's
repository is an argument error before anything starts.

For `mobile-v2`, without an explicit local source or prebuilt artifact, Mortsom fetches `main` from
`https://github.com/MostroP2P/app.git`. `--ref` accepts a branch name or a canonical
`https://github.com/MostroP2P/app/pull/<number>` URL (optional trailing slash). The matching
`refs/heads/<branch>` or `refs/pull/<number>/head` is fetched on every invocation; PRs use the proposal's
head, including fork PRs, never the base branch or synthetic merge. Invalid selectors and missing or
unreachable remote refs fail before building; cached source is never a fallback for a failed fetch.

Reusable Git objects, disposable clean detached build checkouts and kept bundles live under the ignored
`.mortsom/app/` working directory. The requested selector, repository, full ref and fetched commit ID are
recorded in build, run and recovery provenance. One invocation resolves, builds and captures its bundle once;
every selected scenario uses that exact bundle and provenance. A changed stack fingerprint refuses reuse.
Linux and Web source preparation/builds remain sequential. Normal completion or failure removes the
disposable checkout; a killed process may leave ignored build files, never trader recovery state, there.

A managed source build keeps its bundle under `.mortsom/app/bundles/`, keyed by platform, fetched commit,
app package version, observed tool versions and the exact build commands, which carry the daemon identity,
relay address and order expiry. Every invocation still fetches, checks out the selected head and observes
the tools. When the key matches a kept bundle whose recorded fingerprint still verifies, that bundle is
captured instead of building and the build provenance has origin `cached` with the original build's facts. A
kept bundle that fails verification or the artifact checks, which its fingerprint does not cover (such as a
Linux executable that lost its mode), is discarded and the source is built. The key sees nothing of the host
beyond those tool versions, such as compiler flags in the environment or upgraded system libraries;
`run --rebuild` covers such a change, building even on a match and replacing the kept bundle. It conflicts
with `--artifact`. At most four bundles are kept; the least recently used are evicted after a store.
Local checkouts and prebuilt artifacts are never kept or reused. Failing to read or keep a bundle is logged
as a warning and falls back to, or keeps, the fresh build.

`mobile.app_ref` configures the same selector; omitting it selects `main`. `mobile.source_dir` selects an
explicit developer checkout and conflicts with `mobile.app_ref`. `--source` and `--ref` override the
configured source mode. They conflict with each other and with `--artifact`. Prebuilt artifacts bypass
source fetching/building. Existing local-source configurations remain supported, including their dirty state.
`doctor` checks installed tools without fetching or building remote source and marks source-contract checks
as deferred until `run` fetches it. Neither command changes the daemon version or stack lifecycle.

Mortsom builds app v2 from `lib/main_mortsom.dart` with a test-environment marker, the daemon's public
identity and the local relay seeds. Production relay fallback is disabled before any subscription starts.
Advertised relay addresses are validated from the client's own network namespace.

Builds resolve Dart packages with `flutter pub get` and follow the app's supported scripts. A managed checkout
whose revision tracks its Flutter Rust Bridge bindings (`lib/src/rust/frb_generated.dart` and
`rust/src/frb_generated.rs`) is built with them as committed, as the app's own release is; a local checkout,
or a revision that does not track them, has them regenerated with `scripts/frb-generate.sh` before the build.
The build provenance lists the commands that ran. Generated bindings are never edited by hand. Linux and Web
builds run sequentially because binding generation and the build write into the shared source tree.

Mortsom records the source revision, dirty flag, build flags, tool versions and the bundle digest. Both actors
use the same artifact. A prebuilt artifact or kept bundle must pass the same visible environment and identity
checks as a fresh build.

A `cli` run builds `main` of `https://github.com/MostroP2P/mostro-cli.git` unless told otherwise.
`--ref` accepts a branch name or a canonical `https://github.com/MostroP2P/mostro-cli/pull/<number>`
URL, fetched like an app ref into the ignored `.mortsom/mostro-cli/` cache and built detached and clean;
`--source` builds a local checkout, dirty state included; `--artifact` takes a prebuilt
executable. The three conflict. The build is `cargo build --release --locked --bin mostro-cli` with a
shared, ignored target directory under `.mortsom/mostro-cli/`, so a repeated or nearby commit compiles
incrementally; the provenance records the selector, fetched commit, dirty flag, package version, `rustc`
and `cargo` versions and the command. The CLI build is a bundle of two files, the executable and
`mortsom-cli.json` (the daemon key and run relay it is pointed at), captured and fingerprinted like an app
bundle: recovery reopens the CLI against the same daemon. Its discovery must offer every command the
profile needs before an actor starts. `doctor --profile cli` checks a prebuilt executable's discovery and,
for a source selection, Git and Cargo, deferring the discovery to the build.

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
`cancel_before_taken`). `mortsom list` prints every scenario with its tags and requirements. `run` with
neither scenario names nor `--tag` selects every registered scenario, in registry (alphabetical) order. Every
selection runs one scenario per run, each with fresh actors; an unknown name or a tag no scenario carries is
an error, never a smaller run.

| Family | Tag | What it covers |
|---|---|---|
| Happy path | `happy-path` | Trades that work: sell, buy, range (sold and bought), market price, rating |
| Cancellations | `cancellation` | Orders ended before the sats move — unilateral, abandoned, cooperative |
| Expirations | `expiration` | The daemon's own clocks reaping what nobody touched |
| Disputes | `dispute` | Disputes opened, resolved by a solver, or undone by the parties themselves |
| Payment failures | `payment-failure` | Invoices the daemon cannot pay, how it recovers, and how many unresolved payouts it lets one payee pin |
| Recovery | `recovery` | An app relaunched mid-trade shows the trade again and repeats no action; interruptions of the client or the relay |
| Anti-abuse bonds | `bond` | Bonds released, slashed and claimed |
| Adversarial | `adversarial` | Malformed, replayed, out-of-state and unauthorised messages |

This version implements the first five families and the anti-abuse bonds (section 8.8) in full, and of the
recovery family the relaunches (section 8.7); its other scenarios need interruptions Linux and Web clients
cannot cause and are skipped. The adversarial family is roadmap (section 14): its tag is reserved, and no
scenario carries it yet.

The bond family is written for a daemon that asks for bonds, and every other family for one that does not:
a bonded daemon parks each order and each take on a bond those scenarios never pay, and a bondless one
never opens the pay-bond screen the bond family waits for. A run decides from the bond the stack declares
(section 5) and skips, with the reason, every scenario written for the other kind of daemon; without a
Mortsom-owned stack nothing declares a bond, and only the bond family is skipped. One stack therefore
accepts either the bond family or the rest, and the operator switches with `stack up --bond --replace` or
`stack up --replace`, each of which mints a new daemon identity and so a new client build.

A scenario whose profile lacks a required capability is **skipped with a reason, never failed**, and a
skipped scenario is never counted as accepted. Three capabilities exist for what a client may not offer:
`offered-actions` (the app shows only the trade actions a state allows; the CLI offers every command and
lets the daemon refuse), `order-expiry` (a test build asks the daemon for the stack's short order
lifetime) and `invoice-check` (the app judges a buyer invoice against the node's limits before sending
it; a client without it sends the invoice and leaves the verdict to the daemon). A scenario that needs
`offered-actions` or `order-expiry` declares it; one that meets a client without `invoice-check`
expects the daemon's verdict instead of the app's.
The same holds for a scenario that needs an interruption the run cannot cause: sending the app to the
background, cutting its network, rotating the device, or restarting or replaying into the run relay. Linux
and Web clients cause none of them, and a run's relay is the stack's.

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
| `cancel_before_taken` | Alice creates a sell order and cancels it while pending. | The relay shows the order `canceled`; Alice's app then no longer holds it open. No payment. |
| `cancel_after_take_unpaid` | Alice creates a sell order. Bob takes it and never provides an invoice. | After the daemon's taker window it publishes the order as pending again, cancelled or expired — a revision later than the take, never the original `pending`. Alice's app shows it pending again, or no longer holds a closed one open. A republished order is closed by the cleanup sweep. |
| `taker_cancel_at_taken` | Alice creates a sell order. Bob takes it, provides his invoice and cancels before Alice pays. | The relay republishes the order as pending after the cancel; Alice's app shows it pending. The cleanup sweep closes it. |
| `cooperative_cancel` | Alice creates, Bob takes, Alice funds. Alice asks to cancel; Bob agrees. | Bob's app keeps the order active for the whole quiet window after Alice's request. After Bob's agreement both apps and the relay show it cancelled and Alice's hold payment fails (the escrow returns). |
| `unilateral_cancel_rejected` | As `cooperative_cancel` up to Alice's request; Bob does not agree during the window. | Bob's app keeps the order active and the relay publishes no `canceled` for the whole quiet window. Bob then agrees, which returns the escrow; that step is teardown, not the assertion. |

The maker's own screen is never proof of a cancellation: app v2 marks a funded order cancelled on the
request alone, while the daemon has only opened the cooperative flow and still holds the escrow.

An order cancelled before it ever went active (pending, or waiting for an invoice or a payment) leaves
app v2's My Trades instead of reading cancelled there. The daemon's `canceled` on the relay is the proof;
the app's evidence is that it no longer holds the order open: shown closed, or no longer listed. A missing
row counts only on a list the app shows as loaded (a row, `trades.empty`), never while it is loading or on
`trades.error`. The range remainders of section 8.2 are closed the same way.

### 8.4 Expirations

Every scenario here waits for a deadline the daemon owns, so each takes as long as the stack's limits
(section 5, `stack/limits.toml`) say. The proof is the daemon's published state after a baseline taken
before the wait, never a screen. App v2 drops a trade that never went active from its list once the
daemon closes it, so on the app the evidence is that the order is no longer held open: shown closed, or no
longer listed.

| Scenario | Narrative | Required evidence |
|---|---|---|
| `pending_order_expires` | Alice creates a sell order and nobody takes it. The test build asks the daemon for the lifetime the stack declares (`order_expiration_secs`, through `MORTSOM_ORDER_EXPIRY_SECS`), since the daemon's own default is hour-granular. | The relay shows the order closed after the baseline (the daemon's own status is `expired`; the book carries `canceled`, the one wire word for every order closed without a trade). Alice's app no longer holds it open. No payment. |
| `hold_invoice_expires` | Alice creates a sell order, Bob takes it and provides his invoice; Alice never pays the hold. | After the daemon's taker window the relay shows the order `canceled` (never `expired`, which is for orders nobody took). Neither app holds it open. No payment. |
| `buyer_invoice_deadline` | Bob creates a buy order, Alice takes it and pays the hold; Bob never provides his payout invoice. | After the window the relay shows the order `canceled`, neither app holds it open, and Alice's hold payment fails: the escrow came back. |

A fiat-payment window is not driven: the daemon has no such clock (`fiat_window_secs` stays undeclared in
`stack/limits.toml`), so an active order never expires on its own.

### 8.5 Disputes

A dispute is opened from the trade detail by either party of a funded order (active or fiat-sent) and ends
in one of three ways: a solver settles the order for the buyer, a solver cancels it for the seller, or the
seller releases and the daemon closes the dispute itself. A solver is a user the daemon's operator
registers, signed with the daemon's own key. Mortsom owns the stack, so it holds that key: a run that needs
a solver mints a fresh solver identity, registers it with `admin-add-solver` as the operator would, and
rules through the protocol exactly as a solver client does: `admin-add-solver` travels from the daemon key as
identity and trade key alike and carries a message id (the daemon's structural check demands one and drops
the message silently without it); `admin-take-dispute` carries the dispute id and `admin-settle` /
`admin-cancel` the order id, all on the dispute wrapper. The keys stay in the coordinator; a scenario
sees only *take*, *settle* and *cancel*. Without a Mortsom-owned stack such a scenario is skipped with the
reason, never failed. The solver's public key is scoped into the relay observer, so its protocol traffic is
run evidence like the actors'.

The daemon publishes a dispute as its own replaceable event (kind 38386). It names the dispute id, the
dispute status (`initiated`, `in-progress`, `seller-refunded`, `settled`, and `released` when the seller
releases while it is open) and who opened it, and never the order, so the proof of each step is that event published after a baseline taken before the step, and the
order the dispute is about is what the daemon reports when the solver takes it. A disputed order stays
`in-progress` on the book until it closes: the book carries `success` or `canceled` at the end, and never
`dispute`.

| Scenario | Narrative | Required evidence |
|---|---|---|
| `dispute_settled_by_solver` | Alice creates, Bob takes, Alice funds, Bob reports fiat sent; Alice does not release. Bob opens a dispute; the run's solver takes it and settles the order for Bob. | The relay shows the dispute `initiated` by the buyer, then `in-progress`, then `settled`; the solver's take names this order. Both apps show the order disputed, then completed; the book shows `success`. The hold settled once on Alice's node and the payout settled once on Bob's. |
| `dispute_canceled_by_solver` | Alice creates, Bob takes, Alice funds; Bob never pays. Alice opens a dispute; the solver takes it and cancels the order for Alice. | The relay shows the dispute `initiated` by the seller, then `in-progress`, then `seller-refunded`. Both apps show the order disputed, then cancelled; the book shows `canceled`. Alice's hold payment fails: the escrow came back. |
| `dispute_released_by_seller` | As `dispute_settled_by_solver` up to the dispute; then Alice releases anyway. No solver acts. | The relay shows the dispute `initiated` by the buyer and, after the release, `released` (mostro-core's `Status::Released`, kept apart from a solver's `settled`). The trade completes as in `happy_sell`: both apps completed, the book `success`, the hold and the payout each settled once. |

The cleanup sweep treats a disputed order as out of the parties' hands: when the run has a solver, it takes
each open dispute the daemon published until one is about the order, and cancels the order so the escrow
returns; without a solver the order is reported unresolved.

### 8.6 Payment failures

The daemon settles the seller's escrow at release and only then pays the buyer's invoice. What it does when
that payment cannot be made is a product surface of its own: it retries on the schedule it declares
(`[lightning].payment_attempts` every `payment_retries_interval` seconds), tells the buyer with
`payment-failed`, and once the retries are spent asks for a new invoice with an `add-invoice` whose order
still reads `settled-hold-invoice`; the app must show that request as it shows the first one, and the
replacement goes through the same manual form. Before any of that, an invoice the daemon will not be able
to pay in time is refused (`invoice_expiration_window`, declared to the run as
`payout_invoice_min_expiry_secs`): by the app, which judges an invoice against the node's capabilities before
sending it and names the rule it breaks in `invoice.check`, or, while those capabilities have not reached
the app, by the daemon at submission. And a payee who never settles what the daemon pays holds an HTLC open
on the daemon's node for each payout; the daemon caps how many unresolved payouts one destination may
hold (`max_inflight_payouts_per_destination`) and lets the next one wait, settled escrow and all, until a
slot frees.

Bob's wallet is a node Mortsom holds, so every invoice in this family is shaped from the outside and never
through the app: withdrawn on the node before the release, issued with a chosen lifetime, or issued as a
hold invoice. The declared limits reach scenarios from the stack descriptor; a stack that declares none
makes the scenario an environment error before an order exists. A withdrawn or refused invoice is
recorded as superseded in the recovery journal before the node cancels it, which is what lets a
replacement be prepared for the same order and what recovery later verifies: a superseded payout must
show failed with no settled record, whatever the order came to.

| Scenario | Narrative | Required evidence |
|---|---|---|
| `payout_fails_then_replaced` | Alice creates, Bob takes and invoices, Alice funds, Bob reports fiat sent. Bob withdraws his invoice on his node. Alice releases. | The hold settles once on Alice's node at the release. The daemon fails to pay, retries as declared, then asks Bob for a new invoice: his app shows `waiting-invoice` again. Bob hands in a replacement through the manual form; both apps show completion, the book `success`, the replacement settled once on Bob's node and the withdrawn invoice failed with no settled record. |
| `payout_invoice_below_expiry_floor` | Alice creates, Bob takes and hands in an invoice whose lifetime is half the declared floor. | It is refused, and the verdict is waited for, never read once. Bob's app refuses it before sending it: `invoice.check` reads `expires-too-soon` and `invoice.submit` stays disabled. While the node's capabilities have not reached the app it cannot judge the expiry, and the daemon refuses it instead (`CantDo`): the form stays open with the reason as `invoice.error`. Either way the order stays taken and waiting for his invoice on both sides. A proper invoice is then judged `valid` by the app and accepted: both apps taken, the book `in-progress`. The refused invoice never settles. No payment; the cleanup sweep closes the order. |
| `payout_slots_capped_per_destination` | One more funded trade than the declared cap, each with a hold invoice on Bob's node as the payout invoice, all reported fiat sent. Alice releases them in turn. | Each release settles that hold once on Alice's node. Up to the cap, each hold invoice on Bob's node turns `accepted`: the payout is in flight and held. The next one stays `open` for the whole quiet window: nothing was dispatched to it. Bob settles the first; its payout lands, and the held-back payout is dispatched on its own (its invoice turns `accepted`). Bob settles the rest; every payout settles once, every trade completes on both apps and on the book. |

Rules for every scenario:

- Every wait has a predicate, a deadline and a failure category. No fixed sleeps.
- Visible states are mapped explicitly from `mostro-core` types. Loading, payout pending, escrow settled,
  payout completed, cancellation and expiration never collapse into one success state.
- A form-derived observation must carry the exact public order ID. A foreign form cannot authorize a payment.
- Negative assertions observe an explicit interval after the action, with a sequence cursor.
- Private app keys are never extracted to decrypt messages. Evidence comes from the UI, public relay events
  and wallet results. On a stack Mortsom owns, the daemon key opens the actors' messages to the daemon so a
  read-only query is told from a trade action (section 8.7); only the action is kept, never the content.

New scenarios are added one family at a time, each scenario with its own row here and its own live
acceptance per platform. The earlier twenty-scenario catalogue remains a roadmap in
[docs/history/spec-v0.3.md](docs/history/spec-v0.3.md), section 10.

### 8.7 Recovery

| Scenario | Narrative | Required evidence |
|---|---|---|
| `restart_at_pending`, `restart_at_taken`, `restart_at_active`, `restart_at_fiat_sent`, `restart_at_completed` | The trade reaches that state; the app is force-stopped and relaunched (the maker's alone while pending, both apps otherwise). A trade relaunched while active or fiat-sent then completes as `happy_sell`; one relaunched while pending or taken is cancelled by cleanup. | Each relaunched app shows the trade in the same state, and no actor sends the daemon a trade action during the quiet window after the relaunch. |

An app makes read-only queries on any launch: its own reputation (`user-info`), a session restore, its
orders. They are not actions. On a stack Mortsom owns, the run opens each actor message to the daemon with
the daemon key and keeps its action, never its content; a query does not count. Without the daemon key
every actor message counts as an action, and the run records that it could not tell them apart.

### 8.8 Anti-abuse bonds

A daemon started with `[anti_abuse_bond]` asks for a second hold invoice, the bond, from whichever side its
policy names; the stack (`stack up --bond`) bonds both. A maker's bond is asked for right after `new-order`
and gates publication: until it locks the daemon publishes nothing, and past its payment deadline it drops
the order unpublished. A taker's is asked for right after the take: the daemon parks the order at
`waiting-taker-bond` for that taker alone, the maker hears nothing and the book keeps the order `pending`, so
a taker who never pays blocks no one; the take reaches the book as `in-progress` only once the bond locks.
On every honest exit the daemon cancels the bond's HTLC, so the sats never leave the payer's wallet. A bond
is slashed only by a solver's `BondResolution` on `admin-settle` or `admin-cancel`, or by the scheduler when
a waiting step outlives `expiration_seconds` with `slash_on_waiting_timeout` on: `waiting-buyer-invoice`
slashes the buyer, `waiting-payment` the seller, maker or taker alike. A taker's failure puts the order back
on the book; a maker's closes it, returning the other side's bond and any escrow. The slashed HTLC settles,
the node keeps its share (`slash_node_share_pct`, floored) and offers the rest to the winning counterparty
with `add-bond-invoice`, asked again every `payout_invoice_window_seconds` until an invoice arrives, which
it pays; one it cannot pay within its retries is asked for again.

Nothing about a bond is public beyond publication itself: kind 38383 carries no bond and no event marks a
slash or a payout. The proof is in the wallets, which Mortsom holds: a bond that came back is a payment that
failed with no settled record, a slashed one a payment settled once for exactly the bond, a claim an invoice
on the winner's node settled once for exactly the share. The run sizes each against the declared policy:
the bond is `max(round(amount_pct × order sats), base_amount_sats)`, which every bond invoice must ask
exactly, and the share is the bond less the node's floored share, which the app must offer exactly. Every
order here is priced in sats, so both are known before an invoice exists.

Bonds are paid and claims invoiced from outside the app, from the actors' nodes. The pay-bond screen hands
the bolt11 over (`bond.order_id`, `bond.invoice.text`), and the app moves on by itself once the daemon sees
the payment; `bond.cancel`, confirmed with `bond.cancel.confirm`, walks away: a taker's cancel of the take,
a maker's cancel of an order never published. A winner reads the claim (`bond.claim.amount`,
`bond.claim.status`) from the trade's banner (`trade.bondClaim`) or from the claim-only row a closed trade
leaves, and hands in an invoice through `bond.claim.text`. A loser's trade carries `trade.bondSlashed` with
the cause while the row exists; a timeout wipes a never-active row, and then only the wallet speaks.

| Scenario | Narrative | Required evidence |
|---|---|---|
| `taker_bond_released_after_trade` | Alice creates and bonds a sell order. Bob takes it, pays his bond, and the trade completes as `happy_sell`. | Alice's app shows the order on its bond until it locks, and the book carries it only then. After the take Bob's app shows the bond window, Alice's still shows the order pending, and the book publishes no `in-progress` for the whole quiet window; once his bond is paid it does. Each bond invoice asks exactly what the policy sizes it at. Hold settled once, payout settled once, both apps completed, and both bonds fail on their nodes with no settled record. |
| `seller_taker_bond_then_escrow` | Bob creates and bonds a buy order. Alice takes it as seller: the bond first, then the escrow, each a hold invoice paid from her node. The trade completes as `happy_buy`. | The bond evidence above with the roles reversed. After her bond locks Alice's app asks for the escrow; after the escrow is accepted Bob's asks for his invoice. Hold settled once, payout settled once, both bonds returned. |
| `taker_bond_declined` | Alice creates and bonds a sell order. Bob takes it, sees the bond window and walks away. Alice cancels the order. | Bob's app no longer holds the order; Alice's keeps it pending; the book publishes neither `in-progress` nor `canceled` for the whole quiet window. After Alice's cancel the relay shows it cancelled, her app no longer holds it open and her bond comes back. Bob paid nothing. |
| `taker_bond_expires_unpaid` | As `taker_bond_declined`, but Bob neither pays nor walks away; he is on the trade screen, not the pay-bond screen, when the bond lapses. | The order never leaves `pending` on the book while the bond is unpaid: the daemon publishes no `in-progress`, so others can still take it. After the bond invoice's lifetime the daemon republishes it, a new `pending` revision later than the take. Alice's app shows it pending throughout. The daemon tells Bob nothing; his app knows the invoice's expiry and no longer holds the order within `state_timeout` of the revision. Bob paid nothing; Alice's cancel returns her bond. |
| `maker_bond_locked_before_publish` | Alice creates a sell order and is asked for a bond before it is published. She pays from her node, then cancels the order. | Her app parks the order on the bond (`waiting-bond`) and the book carries nothing for the whole quiet window. Once paid, her app and the book show it pending. After her cancel the relay shows it cancelled, her app no longer holds it open, and her bond fails on her node. |
| `maker_bond_abandoned` | Alice creates a sell order, is asked for a bond, and walks away on the pay-bond screen. | Her app no longer holds the order; the book never carries it. No payment. |
| `maker_bond_expires_unpaid` | Alice creates a sell order, is asked for a bond, and neither pays nor walks away. | Her app keeps the order on its bond for the whole quiet window, and no longer holds it once the maker's deadline passes; the book never carries it. No payment. |
| `bonds_released_by_solver_ruling` | Alice's bonded sell order is taken with Bob's bond, funded and reported fiat sent; Alice disputes. The run's solver takes the dispute and settles it for Bob with a resolution that slashes nobody. | The dispute goes `initiated`, `in-progress`, `settled`; both apps and the book show completion; hold settled once, payout settled once, both bonds returned. |
| `buyer_taker_bond_slashed_on_timeout` | Alice's bonded sell order; Bob takes it, pays his bond and never hands in his invoice. Alice claims the share, then cancels the order. | After the buyer's deadline the book publishes the order `pending` again. Bob's bond settles on his node and his app no longer holds the order. Alice's app offers a claim for exactly the share, reading `pending`; an invoice from her node for it is handed in, settles once, and the claim reads `completed`. Her cancel returns her own bond. |
| `seller_maker_bond_slashed_on_timeout` | Alice's bonded sell order; Bob takes it, pays his bond and hands in his invoice; Alice never funds. Bob claims the share. | After the seller's deadline the book shows the order `canceled`. Alice's bond settles on her node and her app no longer holds the order; Bob's bond comes back. Bob's claim is offered, paid and completed as above. |
| `seller_taker_bond_slashed_on_timeout` | Bob's bonded buy order; Alice takes it, pays her bond and never funds. Bob claims the share, then cancels the order. | After the seller's deadline the book publishes the order `pending` again. Alice's bond settles on her node and her app no longer holds the order; Bob's shows it pending. His claim is offered, paid and completed, and his cancel returns his own bond. |
| `buyer_maker_bond_slashed_on_timeout` | Bob's bonded buy order; Alice takes it, pays her bond and the escrow; Bob never hands in his invoice. Alice claims the share. | After the buyer's deadline the book shows the order `canceled`. Bob's bond settles on his node and his app no longer holds the order; Alice's escrow payment and her bond both fail on her node (returned). Her claim is offered, paid and completed. |
| `taker_bond_slashed_by_solver` | As `bonds_released_by_solver_ruling` up to the ruling; the solver cancels the order for Alice with a resolution that slashes the buyer. Alice claims the share. | The dispute goes `initiated`, `in-progress`, `seller-refunded`; both apps and the book show the order cancelled and Alice's escrow fails (returned). Bob's bond settles on his node and his trade shows `trade.bondSlashed` as `dispute`. Alice's claim is offered, paid and completed, and her own bond comes back. |
| `maker_bond_slashed_by_solver` | As `bonds_released_by_solver_ruling`, but Bob opens the dispute, and the solver settles it for Bob with a resolution that slashes the seller: Alice, the maker. Bob claims the share. | The dispute goes `initiated` by the buyer, `in-progress`, `settled`; both apps and the book show completion; hold settled once and payout settled once. Alice's bond settles on her node and her trade shows `trade.bondSlashed` as `dispute`; Bob's comes back. Bob's claim is offered, paid and completed. |
| `bond_claim_fails_then_replaced` | As `buyer_taker_bond_slashed_on_timeout` up to the claim; Alice hands in a hold invoice from her node and withdraws it once the daemon's payment is held there. She then claims with an ordinary invoice. | The withdrawn invoice is recorded superseded before the node cancels it, and never settles. The daemon, its retries spent, asks again: Alice's app reads the claim `pending` once more. The replacement settles once for exactly the share and the claim reads `completed`. |

A scenario reached without a declared bond, or with a policy that lacks what it drives (a maker's bond,
timeout slashing), ends as environment misconfiguration before an order exists. Not covered, and on the
roadmap (section 14): the lock race between two takers, which needs a third actor; a claim left to forfeit
and a slash the node keeps whole, which need other policies than the stack's; and a range maker's bond,
sized from the market price at creation, which the run cannot predict.

## 9. Identity and isolation

Each run starts with fresh app storage and new generated identities for Alice and Bob. The run observes both
public identities through the UI, proves they differ, relaunches each app and proves each identity persists.
Both actors bootstrap at once, each on its own isolated session (section 6.1); the report lists Alice's steps,
then Bob's, whichever finishes first. Mnemonics and private keys are never exported.

Observers start before the actions they verify. A run refuses to create an order when the daemon identity
does not match, the relay is not ready, the daemon advertises maintenance mode or actor bootstrap failed.
The preflight's `daemon-maintenance` check reads the `maintenance_mode` tag of the daemon's info event.
`true`, or any value but `false`, fails it as `environment-misconfiguration`, naming the likely cause (a
stale chain tip) and the fix (mine a block, wait five minutes). A daemon that predates the tag passes.
Only one run may use a given stack and
wallet set at a time; a second run reports the conflict before launching anything.

## 10. Value safety

These rules protect real funds, even on regtest, and are never relaxed for convenience:

- Reserve budget before any payment: the actual invoice amount plus the fee ceiling. Per-order and cumulative
  caps are mandatory; a missing cap is an error. Retries and resets never reset the cumulative budget. The
  per-order cap bounds every reservation an order holds together: the maker's anti-abuse bond, the taker's
  and the escrow on one order count as one spend, each with its fee ceiling, and a bonded trade checks they
  fit before it pays any of them.
- Caps and the mainnet guard bind each run. A `run` that selects several scenarios (a tag, or the whole
  catalogue when nothing is named) is a sequence of runs, each cleared and capped on its own, so it can spend
  up to its value-moving scenarios times `max_run_spend_sats`; it says how many scenarios it starts, and how
  many move sats, before the first one.
- Write durable submission or payment intent before the irreversible UI or RPC action.
- Never blindly resubmit an order after a missing confirmation, and never retry a payment whose outcome is
  uncertain. An invoked payment without a terminal result stays in flight until accounting succeeds.
- Look up payments by hash in both directions: an invoice this node issued and an outgoing payment it made.
  A node that has never issued an invoice must not turn an outgoing payment into "unknown".
- Network clearance comes from the observed wallet and node networks. `localhost` does not prove regtest.
  Explicit mainnet confirmation remains fail-closed.
- Secrets use `env:` references, become `Secret` values immediately and never appear in arguments, build
  defines, logs or artifacts. Invoices handed to the CLI travel on its standard input (`--invoice -`).

## 11. Cleanup and recovery

Cleanup is registered as soon as a submission can create an order, including uncertain submissions. New
run-owned order IDs are discovered from UI and relay observations so a missed confirmation does not orphan an
order.

On success, failure or interruption, in order:

1. Preserve sanitized UI, relay and wallet evidence.
2. Resolve or cancel every tracked order through the normal UI. `pay.cancel` cancels the order; it is not a
   navigation shortcut. A trade the maker's loaded My Trades no longer lists is not opened again: the
   relay decides whether the daemon closed it (8.3).
3. Verify terminal wallet outcomes; record outstanding hashes and escrow. An order's terminal state is read once,
   from its loaded list first, and holds for every payment on it; each payment is still judged by its own
   hash. A screen that cannot show the order yet (app v2's `loading`) is read again within the state
   timeout, never taken as the order's state. A payment is journaled with what it
   pays for. A bond is judged by its own end, whatever the order came to: returned (failed, nothing settled)
   or settled once for exactly the journaled amount, never in flight. A claim of a slashed bond's share is
   never cancelled by recovery, since it settles on an order that did not complete, and only a claim settled
   once for exactly the share is at rest: a claim invoice that failed without the scenario withdrawing it
   leaves the daemon asking for a replacement, which only the retained app can hand in. `cleanup <run-id>` runs on a bonded
   stack and a bondless one alike, whatever family left the journal. An actor's
   bond and escrow on one order, or their payout and their claim, are two intents, never a replay.
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

The CLI runtime (section 6.3) marks each invocation, not a session: the marker is set before the child starts
and cleared once it is gone, by a task that outlives a caller that gives up. A marker left by a killed
coordinator is reconciled by the CLI runtime itself, and only after it checks, through `/proc`, that no
process of the operator's user that `/proc` shows runs with the actor's `MCLI_DIR` (set to the store's path,
or to another absolute path to the same directory, as through a bind mount; a process whose other path cannot
be checked against the store, because the check does not end within a short bound, as on an unresponsive mount,
or that path or the store cannot be read, counts). That needs `/proc` to be the
procfs of Mortsom's own PID namespace, listing every process of the operator's user: a procfs mounted with
`hidepid=invisible` (while that user is not in its `gid=` group) or `hidepid=ptraceable` leaves out every
non-dumpable process of that user, and a procfs of another PID namespace shows none of Mortsom's. `/proc`
proves a listed process free of the store only for a process whose environment it shows, and it hides the
environment of a non-dumpable process even from that process's user. For such a process of the operator's user
(by any of the users its status names or, when `hidepid=noaccess` hides the status too, by the owner of its
`/proc` directory) it can go only by the command line: the process counts as running with the actor's
`MCLI_DIR` when its first argument names an executable `mostro-cli`, or when its command line cannot be read
either. A process that has exited does not count, except a thread-group leader whose other threads still run,
which is judged by those threads. While a process counts, or when `/proc` cannot be read, does not list every
process of the operator's user, or belongs to another PID namespace, the storage stays refused.

A killed run leaves active-runtime markers and later invocations fail closed. Markers are never removed just
to bypass that refusal.

`stack up` and `stack down` are refused while a pending journal's original stack still exists or cannot be
proved gone; `--replace` is never permission to destroy recoverable state. The journal records, at creation,
the facts a later abandonment is checked against: lane, mainnet permission, the Mortsom-owned stack
directory, the artifacts directory the run wrote to, whether both actors used the stack's own LND nodes, and
the network each node reported. Nothing is inferred from the lane name.

`mortsom recovery abandon <run-id> --reason stack-destroyed --confirm-run <run-id>` is the only terminal path
for a journal whose Mortsom-owned regtest stack is provably destroyed. It requires a pending journal, a
recorded environment that is not canary, never allowed mainnet, attached to a Mortsom-owned stack and used
the stack's own regtest nodes, and live proof that the original stack identity is recorded nowhere: not in
the recorded directory, the selected directory or the Compose project. Anything unreadable or unowned fails
closed. Only a journal that recorded these facts and its artifact directory can be abandoned; journals
written before they were recorded are refused, and the selected configuration is never a substitute.
An unresolved legacy journal without `RecoveryEnvironment` has a separate, nonterminal route:
`mortsom run ... --preserve-legacy-recovery <run-id>`. Repeat the option for every pending legacy
journal on every invocation. This leaves the old manifests, events, profiles, bundles and reports
unchanged and pending; it neither proves destruction nor verifies the old environment, orders or
payments. The old runs remain unresolved and unaccepted. No historical fields are backfilled.

Continuation requires exact acknowledgement of the entire pending inventory, readable private journals,
both old actor directories with verified runtime shutdown, and a different current stack fingerprint
compared in the original digest schema (v1 or v2; unknown schemes refuse). Recorded digests must be
exactly 64 lowercase ASCII hexadecimal characters; malformed or noncanonical fingerprints are errors,
never evidence of a different stack, and journals containing them are left untouched. Journals with recorded
environments must use normal cleanup or proven abandonment and cannot take this route. The current
Compose project and descriptor must belong to the selected Mortsom stack, configuration must bind both
actors to that stack's LND endpoints, mainnet permission must be disabled, the lane must not be canary,
and both connected wallets must report regtest. These are facts about the **new** run only. The host lock
is held throughout; inventory and configuration are rechecked before journal creation, and both node
networks are checked again before clients start, including non-payment scenarios. A new pending journal
blocks subsequent scenarios normally. Default runs, mainnet and canary keep the global pending guard.
The new run durably writes `recovery-isolation.json` before client startup and includes a warning naming
every preserved pending run. Its own acceptance still requires ordinary order/payment and cleanup proof.

Schema-1 stack descriptors are read without rewriting them, preserving source provenance as unrecorded.
Their original fingerprint, endpoints, image and limits are validated; schema-2 descriptors still require
source provenance. A schema upgrade alone cannot authorize isolation, abandonment or stack destruction.

Sanitized evidence is written in a fixed order: a prepared `recovery-abandonment.json` in the run's recorded
artifact directory, `abandonment.json` in the journal, the journal turning terminal, the public record
finalized, and retained storage removed last, with every removal reported even when a later one fails.
Reports, accounting, parity, publish and offline cleanup treat any present record, prepared, finalized or
unreadable, as abandonment, so an interrupted abandonment fails closed from its first write; rerunning the
command finishes it, and cleanup that verifies resolution instead discards only a prepared record for
that exact run and stack. Cleanup retries this reconciliation even after the journal is already Resolved;
removal or directory-sync failures return an error. Finalized, unreadable, unsafe or mismatched evidence
is retained and refuses successful cleanup. Abandonment verifies no order and no payment: the run is
failed, is never reported as cleaned up, never reaches parity, and is counted as a leftover in accounting.

## 12. Evidence and reports

Each run writes to ignored `artifacts/<run-id>/`: summary, timeline, per-actor screenshots and accessibility
trees, relay events, wallet payments, cleanup and environment records. Each run that passes its preflight also writes
`timings.json`: how long each phase took (preflight, bring-up, each actor's bootstrap steps with their evidence, the
scenario, the probes that attribute a failure, discovery, the relay quiet window, cleanup, payment verification,
teardown) and, per actor, how many UI requests of each kind the session served, how long they took in total
and at worst, how many failed, and how many the run gave up on before they ended. A phase that did not
run is absent: the probes follow only a failure, the quiet window only a submission left unresolved. Timings
are diagnostics: they decide nothing, and a run whose timings cannot be written is not failed for it. Client
builds log each build command with its duration. Environment records include platform,
app revision and artifact digest, Flutter, Rust, browser and driver versions, stack fingerprint, daemon image
digest and version, declared limits and relay endpoints.

Evidence is sanitized: keys, mnemonics, credentials, NWC URIs and invoice contents never appear. Raw app and
service logs live only in actor-private directories and are removed on teardown. Linux screenshots capture
the actor's virtual display, never the operator's desktop. A CLI actor has no screen: each step keeps the
transcript of the invocations it made (`cli.jsonl`: the command, its arguments with any invoice masked, the
exit code, the outcome status and code, and the outcome data with every invoice and payment request
replaced), and teardown keeps the actor's whole transcript and the CLI's standard error.

`run` prints one report for the whole invocation, after its last run: a row per scenario with its outcome
(`PASS`, `FAIL`, `SKIP`, `FLAKY` for a pass that needed a retry, `STOP` for one a refused run kept from
starting) and the time it took, why under anything that did not pass (and, among several runs, which run
failed), then the counts, the total time, the lane, the artifacts directory and a verdict. A run that fails
after its id exists is a failure that names its run, never a refusal.

A scenario sees the stack only through the apps, so a relay or daemon that goes away mid-scenario surfaces as
whatever the apps did next. Once a scenario has failed, and before cleanup, the run probes, read-only,
whether the stack still answers: the relay's port, a connection to each relay the daemon uses, the daemon
itself, and each actor's wallet. A relay keeps serving a dead daemon's last info event, so the daemon counts
as answering only while that event is no older than two of the publish intervals its own settings declare.
Only signs of life count; the preflight's liquidity floor and its freshness window for any daemon are guards
for starting, which a healthy stack stops meeting in the ordinary course of a run. If two probes in a row find
something not answering, the failure is `test-infrastructure`: its message names what did not answer and
keeps the category and message the scenario reported, and the timeline and `run.log` record the attribution;
nothing is recorded when the failure keeps its category. A failure already classified `test-infrastructure`
is left as reported. A stack can answer and still refuse: if the probes find everything answering but the
daemon's info event advertises `maintenance_mode=true`, the failure is `environment-misconfiguration`, the
category the preflight gives the same daemon. Its message names maintenance mode, the likely cause and the
fix, and keeps the category and message the scenario reported. A stack that stopped answering still makes it
`test-infrastructure`, and a failure already classified `environment-misconfiguration` is left as reported.
The scenario narrative
(timeline steps, checks, payments) goes to the run's `run.log` and `timeline.md`, not the terminal, unless
`RUST_LOG` asks for it.

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
`mostro-core` stays pinned at `=0.17.3` with its Nostr dependency line until a separate reviewed commit
changes it.

Each completed change is reviewed independently before commit and the resulting commit is reviewed for
overlooked bugs. Commits follow Conventional Commits and never include secrets, private state or artifacts.

## 14. Deferred work

Documented as roadmap, not required by this version:

- The adversarial family of section 8.1.
- What the bond family (section 8.8) leaves out: the lock race between two takers (a third actor), a
  claim left to forfeit and a slash the node keeps whole (other stack policies), and a range maker's bond.
- CPU and memory comparison of Linux and Web under matched conditions.
- Multi-scenario runs with in-process profile reset.
- NWC wallet mode for desktop clients.

## 15. Related documents

| Document | Purpose |
|---|---|
| [README.md](README.md) | Installation, quick start and command reference |
| [docs/HANDOFF.md](docs/HANDOFF.md) | Current state, open work and how to resume |
| [docs/acceptance-ledger.md](docs/acceptance-ledger.md) | Per-platform acceptance ledger |
| [docs/runbook.md](docs/runbook.md) | Failure categories and operator responses |
| [docs/cli-contract.md](docs/cli-contract.md) | The mostro-cli machine interface the `cli` profile relies on |
| [docs/history/](docs/history/) | Archived material: spec v0.3 with its plan and attempt ledger, the app v2 inventory and the crate rationale (both written when the emulator path still existed) |
