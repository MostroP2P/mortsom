# Legacy Android setup

The following instructions describe the existing Android workflow. Select `--platform android` and the
legacy configuration explicitly for scenario runs or doctor; this workflow is outside the current checkpoint.
Run the examples from the Mortsom repository root. The current default configuration is the desktop
example, so copying `mortsom.toml` does not select it: legacy operations below pass `--config mortsom.toml`.

### What Mortsom is doing, in one picture

Mortsom is not a client. It drives **two real installations of the app** on two
Android emulators — Alice sells, Bob buys — and watches the trade from three
independent vantage points: what the screens show, what the relay carries, and
what the wallets actually paid. A scenario passes only when all three agree.

That means a run needs more than this repository. The split is worth learning
once, because almost every confusing failure comes from the right-hand column:

| Mortsom creates and destroys per run | You provide and keep running |
|---|---|
| the Nostr relay (a Docker container) | the Mostro daemon (`mostrod`) |
| both emulators, the app installs, the Appium sessions | the app checkout it builds from |
| the run directory with all evidence | two funded NWC wallets, one per actor |

Nothing in the left column outlives a run. There is no relay to start, no
emulator to keep warm; the commands below bring up what they need and tear it
down again, whether they pass or fail.

### Before the first run

You need, on this machine:

- **Docker**, for the relay container.
- **The Android SDK** (`ANDROID_HOME`), with the emulator, `adb`, `avdmanager`
  and the system image the config pins.
- **Appium 3** with the UiAutomator2 driver, and a **JDK**.
- **Flutter**, to build the app under test.
- **A checkout of the app** next to this one (`../mobile` by default).
- **A Mostro daemon you control**, running locally, with its `settings.toml`
  reachable and its relay list including `ws://localhost:7000`.
- **Two wallets**, one for Alice and one for Bob, each funded above the spend
  caps you configure. They are separate wallets on purpose: the run proves who
  paid. Either backend will do — NWC, or two LND nodes on a local
  [Polar](https://lightningpolar.com/) network. See
  [Choosing a wallet backend](#choosing-a-wallet-backend); for a local setup,
  Polar is the one you want.

`mortsom doctor` checks every one of these and names what is missing, so run
it first and read it rather than working from this list.

### Configure

```sh
cp mortsom.toml.example mortsom.toml       # non-secret baseline, gitignored copy
export MORTSOM_MOSTRO_PUBKEY='<daemon pubkey, hex or npub>'
export MORTSOM_MOSTRO_SETTINGS_FILE=~/.mostro/settings.toml
export MORTSOM_ALICE_NWC='nostr+walletconnect://…'
export MORTSOM_BOB_NWC='nostr+walletconnect://…'
```

Secrets (wallet credentials, the daemon settings path) are accepted only as
`env:` references, in every configuration layer including the gitignored
`mortsom.local.toml` overlay; the loader reports every missing variable at once.

Legacy runtime operations such as `device smoke`, `app setup` and scenario runs resolve the whole
configuration, including the four references above. Doctor reports missing references. Offline
`cleanup`, `parity` and `accounting` use only their required nonsecret settings; `publish` still resolves
wallet credentials to check redaction. Export runtime references once per shell, or keep them in a file
you source — never in the repository or literally in `mortsom.toml`.

**The app under test comes from a checkout by default.** `mobile.source_dir`
points at it and every command that installs the app builds it with Flutter,
so nothing else is needed. Point `mobile.apk_path` at a file instead when you
already have the exact build you want to test — a CI artifact, or a build you
are bisecting — and Mortsom installs that file untouched rather than building:

```toml
[mobile]
source_dir = "../mobile"   # built when apk_path is empty
apk_path = ""              # a prebuilt .apk to install as-is
```

**Set the spend caps to what you are willing to lose.** Mortsom moves real
sats on mainnet, and the preflight refuses to start unless each wallet holds at
least the larger cap:

```toml
[run]
amount_sats = 500          # size of the trade a scenario opens
max_order_sats = 500       # refuses any single order above this
max_run_spend_sats = 2000  # refuses to keep going past this in one run
```

### Climb the ladder

Each step brings up more of the system than the one before, and each tears
everything down when it finishes. Only move up when the step below is clean —
a failure at the bottom is a failure you can read.

| | Command | What it proves | Costs sats |
|---|---|---|---|
| 1 | `cargo run -- --config mortsom.toml doctor --platform android` | the host, the secrets, the daemon settings, the wallets | no |
| 2 | `cargo run -- --config mortsom.toml relay smoke` | the relay starts and **your daemon publishes to it** | no |
| 3 | `cargo run -- --config mortsom.toml device smoke` | both emulators boot and the app installs and launches | no |
| 4 | `cargo run -- --config mortsom.toml app setup` | both apps onboard through the UI and connect their wallets | no |
| 5 | `cargo run -- --config mortsom.toml run restart_at_pending --platform android` | a whole scenario, orders and all | no |
| 6 | `cargo run -- --config mortsom.toml run happy_sell --platform android` | a complete trade, paid for real | **yes** |

`cargo run -- list` tells you which scenarios move value; step 5 can be any
scenario marked otherwise.

Mainnet value-moving runs additionally require `run.allow_mainnet = true` and
`MORTSOM_MAINNET_CONFIRM=I_ACCEPT_MAINNET_SPEND`; choosing an Android client does not bypass those guards.

### Reading the output

Every one of those commands ends in the same table. The four verdicts mean
different things, and only one of them stops you:

- **PASS** — checked and satisfied.
- **FAIL** — a requirement is not met; fix it before climbing further. This is
  the only verdict that makes a report `NOT healthy`.
- **WARN** — usable, but worth your attention.
- **SKIP** — not evaluated, because there was nothing to evaluate it against.

`SKIP` is the one that surprises people. `mortsom doctor` starts nothing, so
between runs it has no relay to talk to and skips everything that depends on
one — that is the normal state, not a problem. The command that does prove the
relay and the daemon's use of it is `mortsom relay smoke`.

### Watching a run

Runs are headless by default and the emulators exist only while a run does,
which makes a passing run silent and a failing one hard to picture. To watch
Alice and Bob work, mirror their emulators. Start this **before** the run, in
another terminal: it waits for the devices and attaches as they come up.

```sh
export ADB=$ANDROID_HOME/platform-tools/adb
booted() { $ADB -s "$1" shell getprop sys.boot_completed 2>/dev/null | tr -d '\r'; }

for s in emulator-5554 emulator-5556; do
  (
    $ADB -s "$s" wait-for-device
    until [ "$(booted "$s")" = 1 ]; do sleep 1; done
    scrcpy -s "$s" --no-control --window-title "$s"
  ) &
done
```

`--no-control` matters: it makes the mirror read-only. Without it a stray
click fights the driver mid-scenario and you debug a failure you caused. The
windows close themselves when the run tears the emulators down.

**Use a recent scrcpy.** Mirroring reaches into private Android APIs, so a
version older than the emulator image cannot see the screen at all: against
Android 15 anything before 2.4 dies with `NoSuchMethodException:
SurfaceControl.createDisplay`. Distribution packages lag badly — Ubuntu 24.04
still ships 1.25 — so take it from snap or from the project's releases:

```sh
sudo snap install scrcpy      # or: https://github.com/Genymobile/scrcpy/releases
```

Turning `android.headless` off is the obvious alternative and usually the
wrong one. The emulator's own window depends on the desktop's display server
— under Wayland it goes through Xwayland — and two of them compete for the
GPU with the app under test; a window server that drops one of them fails the
run for a reason that has nothing to do with the app. Mirroring costs nothing
and survives all of that.

Without installing anything at all, two things work everywhere. A screenshot
on demand, refreshed as often as you like:

```sh
$ADB -s emulator-5554 exec-out screencap -p > /tmp/alice.png
```

And the evidence Mortsom writes as it happens:

```sh
watch -n 2 'ls -t artifacts/$(ls -t artifacts | head -1)/*/steps/ | head'
```

Each step leaves a screenshot and a UI dump, so the newest file says where
the run is right now.

### When something fails

Every run writes a directory under `artifacts/<run-id>/`: `timeline.md` (what
happened, in order), `run.log`, `relay-events.jsonl`, and a screenshot and UI
dump per actor at every step. Read the timeline first; it is written for that.
[`docs/runbook.md`](./runbook.md) maps each failure category to what to
look at, and `mortsom cleanup <run-id>` removes what an interrupted run left
behind.

A scenario that fails adds one more step for **both** actors,
`<scenario>-failed`: the screen each phone was on at that moment, whatever
ended the attempt — an assertion, the deadline or a panic. It is the last
entry in the timeline, and usually the one that explains the run. Both
phones are photographed at once and under a short deadline, so a driver
that stopped answering is noted in `run.log` instead of holding the run
open while cleanup waits.

## Choosing a wallet backend

Every run so far has cost mainnet sats and a wallet service's latency. That is
the point of this harness on the canary lane — but it is a slow way to iterate.
A local Polar network on regtest makes a second setup practical, where a whole
trade settles in **under a second of worthless sats**.

Each actor names its own backend, so you can mix them.

| | `kind = "nwc"` | `kind = "lnd"` |
|---|---|---|
| Who holds the wallet | The app. Mortsom types the URI into its settings and the app pays for itself. | Mortsom. The app has **no wallet connected at all**. |
| Who pays the hold invoice | The app, through its connected wallet. | Mortsom, from the node, after reading the invoice off the pay screen. |
| Who issues the buyer invoice | The app. | Mortsom, typed into the app's manual invoice form. |
| What is under test | The app's own NWC integration. | Everything else: the app's manual paths, the protocol, the daemon. |
| Money | Real sats, real latency. | Regtest. Worthless, and fast. |
| Where it fits | The canary lane, and any run whose subject *is* the NWC integration. | Local iteration. |

Both backends are observed the same way: Mortsom asks the wallet that moved the
money whether the payment settled, so value assertions are identical either way.
The spend caps are enforced identically too — a cap that only works on mainnet
is a cap nobody has tested.

### Driving a local Polar network

Create a network with **three** LND nodes and open channels between them.

**The third node is not optional.** `mostrod` must point at a node of its own.
With two, the harness trades with itself: the hold invoice would be paid by the
node that issued it, and every value assertion would be meaningless. Point your
daemon's Lightning backend at the third node before you run anything.

Then copy the config and fill in what Polar's *Connect* tab shows you:

```sh
cp config/polar.example.toml mortsom.toml

export MORTSOM_ALICE_LND_HOST='127.0.0.1:10001'
export MORTSOM_ALICE_LND_CERT=~/.polar/networks/1/volumes/lnd/alice/tls.cert
export MORTSOM_ALICE_LND_MACAROON=~/.polar/networks/1/volumes/lnd/alice/data/chain/bitcoin/regtest/admin.macaroon
# …and the same three for bob, on its own port
```

Paste them exactly as Polar displays them. A host without a scheme is read as
`https://` (LND's gRPC endpoint is TLS-only, so there is no other choice), and
a leading `~` in a credential path is expanded — a config file is not a shell,
and without this the most natural thing to paste is also the thing that fails.

Two things the config refuses, because both fail late and confusingly otherwise:

- **A host with no port.** LND's gRPC port is never the default, so
  `127.0.0.1` alone connects somewhere else instead of failing honestly.
- **A credential path that does not exist.** The network number in a Polar path
  (`networks/1`) changes with every network you create, so a stale path is *the*
  failure mode. Both files are checked when the config loads, and the error
  names the path with the `~` already expanded — the path that was looked for.

Then, as always:

```sh
mortsom --config mortsom.toml doctor --platform android
```

which reports each wallet's liquidity in both directions:

```
[PASS] wallet alice  reachable; lnd backend, alias alice, network regtest,
                     9998735 sats outbound / 9994325 sats inbound, methods […]
```

Inbound matters as much as outbound: a node with no inbound liquidity cannot be
paid, which on a freshly created network is the usual reason a trade dies
halfway through.

## The relay is Mortsom's, not yours

**Do not start a relay yourself.** Unlike a harness you point at existing
infrastructure, Mortsom owns its relay: every run creates a `nostr-rs-relay`
container, uses it, and destroys it when the run ends. There is no
`mortsom relay up`, and that is deliberate — a relay that outlives a run
carries state into the next one, and a test that inherits yesterday's events
is not a test.

What the operator does configure is **the daemon's side**. A relay only
proves anything if the daemon under test publishes there too, so list it in
`~/.mostro/settings.toml`:

```toml
relays = ['ws://localhost:7000', 'wss://relay.mostro.network']
```

Restart the daemon so it republishes its relay list (kind-10002), then prove
the whole chain in one command:

```sh
cargo run -- --config mortsom.toml relay smoke    # start a relay, prove the daemon uses it
```

Keeping the public relay in the list is optional. The local one carries the
run's traffic; the public one keeps orders visible to real clients, which is
what you want against a daemon other people also use. For a fully isolated
run, list only the local relay — and expect the daemon to sit without a relay
between runs.

Three consequences are worth knowing before they look like bugs:

- **`ws://localhost:7000` is Mortsom's port, not a free one.** If something
  else already listens on 7000, the run allocates the next free port and logs
  `configured relay port busy; the daemon must be pointed at the allocated
  port`. A daemon pinned to 7000 would then talk to that other relay while the
  apps talk to the run relay, and the two sides never meet. Leave the port to
  Mortsom.
- **`Connection reset without closing handshake` in the daemon log at the end
  of a run is expected.** That is Mortsom removing its relay. The daemon
  reconnects when the next run brings one up.
- **`mortsom doctor` reports the relay as unreachable when no run is live**,
  and that is correct: `doctor` only looks, it never starts anything. The
  command that proves the relay and the daemon's use of it is
  `mortsom relay smoke`.

## Troubleshooting

Failures worth naming, because each one looked like a different problem than
it was. The evidence for all of them is under `artifacts/<run-id>/`.

### The harness

**`doctor` skips the relay, the relay list and the daemon.** Normal between
runs: `doctor` starts nothing, so there is no relay to ask. `relay smoke`
proves that chain.

**`wallet …: below the run cap`.** The preflight requires each wallet to hold
at least `max(max_run_spend_sats, max_order_sats)` — the worst case a run
could spend, so it cannot stop halfway through a trade. Fund the wallets, or
lower the caps to what you are willing to risk; lowering the caps lowers the
real ceiling too, which is the point.

**A run was interrupted and the next one will not start.** An interrupted run
leaves its relay container, its emulators and its Appium server behind:
`mortsom cleanup <run-id>` removes all three and says what it could not
resolve. `mortsom relay down --all` sweeps containers from runs whose ids you
no longer have.

### The devices

**`The instrumentation process cannot be initialized`** on a session that
never opens. The UiAutomator2 server installed in the AVD is stale or broken,
and Appium keeps it because its version matches. Delete the AVDs and let the
next run recreate them:

```sh
$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager delete avd -n mortsom-alice
$ANDROID_HOME/cmdline-tools/latest/bin/avdmanager delete avd -n mortsom-bob
```

**`emulator … exited during boot: … X connection to :1 broken`.** The
emulator's own window lost the desktop's display server, which is what
`android.headless = false` exposes you to. Keep runs headless and mirror them
instead (see [Watching a run](#watching-a-run)).

**Onboarding times out on an identifier that exists in the app.** Something is
covering the screen — a permission prompt, a legal notice, a confirmation
dialog — and Flutter does not expose what is behind it. Open
`artifacts/<run-id>/<actor>/steps/*-failed-screen.png`: the dialog is in the
picture. If its control carries no automation id, that is a gap in the app's
contract, not a harness bug, and it is fixed in the app.

### The daemon

**`daemon-uses-run-relay` fails although the daemon is reachable.** The
daemon's published relay list (kind-10002) did not reach the run relay in
`mostro.health_timeout_secs`. Two causes, in this order:

- **The window is smaller than the daemon's own interval.** The daemon
  republishes every `publish_relays_interval`, and only after reconnecting to
  a relay that did not exist a moment ago; a window the same size as that
  interval is a coin toss. The shipped configs allow 120s against the usual
  60s interval. Lower the daemon's interval, or raise this.
- **The list is stale**, from before you added `ws://localhost:7000` to its
  `settings.toml`: restart the daemon so it republishes.

**`kind-38385 not yet republished`.** The info event goes out on the daemon's
own interval — `publish_mostro_info_interval`, an hour by default — so a relay
that lives forty seconds rarely sees one. Lower that interval (and
`publish_relays_interval`) while testing, or restart the daemon before a
session. This is a warning, not a gate.

**`Connection reset without closing handshake` in the daemon's log at the end
of every run.** That is Mortsom destroying its relay. Harmless; the daemon
reconnects when the next run brings one up. Keeping a public relay in the
daemon's list as well removes the noise.

### The app checkout

**`Target file "lib/main_mortsom.dart" not found`.** The checkout is on a
branch without the automation contract. The entry point and the identifiers
live on the branch the app's PR adds; switch `mobile.source_dir`'s checkout to
it.

### Mirroring

**`Could not find any ADB device`.** The emulators exist only while a run
does. Start the mirror before the run and let it wait, as
[Watching a run](#watching-a-run) shows.

**`NoSuchMethodException: SurfaceControl.createDisplay`.** The scrcpy is older
than the emulator image. Install a current one; distribution packages are
years behind.

**`command-chain for … gpu-2404-provider-wrapper not found`** from the scrcpy
snap. Its interfaces are not connected yet:

```sh
sudo snap connect scrcpy:gpu-2404 mesa-2404:gpu-2404
```

**An old `scrcpy` runs after installing a new one.** `/usr/bin` precedes
`/snap/bin` on most systems, so remove the packaged one and clear the shell's
cached path with `hash -r`.


See the [current README](../README.md) for Linux/Web, current command defaults, and recovery behavior.
