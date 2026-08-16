# Mortsom

End-to-end test harness for the [Mostro](https://github.com/MostroP2P) mobile
application. *Mortsom* is *Mostro Mobile* written backwards: it watches a real
trade from the user's side of the screen.

Mortsom drives two installations of the app (Alice and Bob) on independent
Android emulators against a locally operated Mostro daemon and a run-owned
Nostr relay, using only what a user can see and do. It is written entirely in
Rust; Appium with UiAutomator2 runs as an external service reached through the
W3C WebDriver protocol.

See [`spec.md`](./spec.md) for the design document, [`AGENTS.md`](./AGENTS.md)
for contributor policy, and [`docs/stack.md`](./docs/stack.md) for the crate
selection.

## Language policy

All code, comments, identifiers, documentation, commit messages and issues in
this project are written in **English**.

## Quick start

```sh
cp mortsom.toml.example mortsom.toml       # non-secret baseline, gitignored copy
export MORTSOM_MOSTRO_PUBKEY='<daemon pubkey, hex or npub>'
export MORTSOM_MOSTRO_SETTINGS_FILE=~/.mostro/settings.toml
export MORTSOM_ALICE_NWC='nostr+walletconnect://…'
export MORTSOM_BOB_NWC='nostr+walletconnect://…'

cargo run -- doctor        # prerequisites, secrets, daemon, relay, wallets
```

`mortsom doctor` performs no payment and creates no order.

Secrets (`MORTSOM_*_NWC`, the daemon settings path) are accepted only as `env:`
references, in every configuration layer including the gitignored
`mortsom.local.toml` overlay; the loader reports every missing variable at once.

## Safety

Mortsom moves real sats on mainnet through the wallets the operator provides.
It refuses value-moving scenarios unless `run.allow_mainnet = true` **and**
`MORTSOM_MAINNET_CONFIRM=I_ACCEPT_MAINNET_SPEND` are both present, and it
never runs without per-order and per-run spend caps.

## Commands

| Command | What it does | Moves value |
|---|---|---|
| `mortsom doctor` | Checks host tools, secrets, daemon settings, relay and wallets. | no |
| `mortsom list` | Lists registered scenarios with tags, requirements and value flag. | no |
| `mortsom relay smoke` | Starts a run-scoped relay, proves the daemon uses it, tears it down. | no |
| `mortsom relay down --run <id> \| --all` | Removes relay containers left behind. | no |
| `mortsom device smoke` | Boots both emulators, installs the app, opens Appium sessions. | no |
| `mortsom app setup` | Onboards Alice and Bob through the UI, connects wallets, relay and node. | no |
| `mortsom run [--tag <tag>] [scenario…]` | Runs scenarios end to end with observers, caps and cleanup. | per scenario |
| `mortsom cleanup <run-id>` | Removes the run's relay container and Mortsom's emulators; reports orders the run left open. | no |
| `mortsom publish <run-id> --out <dir> [--screenshots all\|none]` | Sanitized bundle for CI: redacted text evidence, screenshots by policy, manifest with retry classification. | no |
| `mortsom accounting [--lane <lane>]` | Spend and outcome trend across runs (`accounting.md/json` in the artifacts dir). | no |
| `mortsom parity <run-a> <run-b> [--intentional <toml>]` | Cross-app parity of two runs against different profiles (outcome, state path, relay, wallet, cleanup, contract). | no |

`mortsom cleanup` reads `<artifacts_dir>/<run-id>/cleanup.json` and prints any
order the run's own sweep could not close. It cannot cancel orders itself
(that needs a device session), so it says so instead of reporting success.
An emulator on a Mortsom slot is only shut down when it is Mortsom's AVD.
`mortsom run` takes a host-wide lock (`<artifacts_dir>/.mortsom-run.lock`)
and refuses to start while another run holds it.

## CI and canary

- `ci.yml`: format, clippy, unit tests, coverage floor on every PR.
- `pr-device.yml`: the app's automation-contract widget tests and a real
  two-emulator device smoke on every PR; no daemon, wallet or secret.
- `integration.yml`: the smoke suite on a secured self-hosted runner
  (`integration` environment) with dedicated wallets and the caps of
  `config/ci.example.toml`, one job per application profile plus the
  cross-app parity comparison; on pushes to `main` and manual dispatch.
- `canary.yml`: one scheduled, hard-capped run per day on the canary runner;
  single-run locking, no retries, an issue on failure, accounting trend and
  screenshot-free evidence bundle every time.
- `ci/Dockerfile`: the pinned runner image (Rust, Android SDK, emulator
  image, Appium/UiAutomator2, Flutter, Docker CLI).

See [`docs/runbook.md`](./docs/runbook.md) for failure handling, cleanup
recovery, secret rotation and incident response.

## Application profiles

`mobile.profile` selects the adapter: `mobile-legacy` (`../mobile`, package
`network.mostro.app`, `mortsom.toml.example`) or `mobile-v2` (`../mobilev2`,
package `foundation.mostro.app`, `config/mobile-v2.example.toml`). Both
adapters expose the same `AppAdapter` contract; scenarios never see either
app's identifiers or copy. The v2 inventory and the automation contract the
v2 app must adopt are in [`docs/mobile-v2-inventory.md`](./docs/mobile-v2-inventory.md);
until it does, a v2 run fails at onboarding with an `automation-contract`
failure rather than a false pass. Scenarios tagged `parity` run against
both profiles in separate jobs and `mortsom parity` compares them.

## Scenario catalogue

`mortsom list` is authoritative. Scenarios marked *value* pay a real hold
invoice and always finish (or cooperatively cancel) their trade.

| Suite | Scenarios |
|---|---|
| smoke | `happy_sell` (value) |
| recovery / restart | `restart_at_pending`, `restart_at_taken`, `restart_at_active` (value), `restart_at_fiat_sent` (value), `restart_at_completed` (value) — force-stop and relaunch, same state restored, no repeated protocol message |
| recovery / interruption | `background_at_taken`, `network_loss_at_taken`, `rotation_at_taken`, `relay_restart_at_taken`, `duplicate_events_at_taken`, `stale_event_at_taken` |
| recovery / wallet | `nwc_cancel_payment`, `wallet_unavailable_then_pay` (value) |
| regression / cancel | `cancel_pending`, `taker_cancel_at_taken`, `cooperative_cancel` (value), `expiration_unpaid` |
| regression / permissions | `unauthorized_actions_at_taken` |

Rejected, expired and failed-payout invoices need a controllable wallet
provider (spec §6) and are not modelled against real NWC wallets.
Interruption timing (`run.quiet_window_secs`, `run.background_secs`,
`run.outage_secs`, `run.expiration_timeout_secs`) is configuration.
