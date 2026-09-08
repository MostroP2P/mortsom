# Stack selection (spec §12, Phase 0.1)

Mortsom is Rust only. This document records which crates fill each role and
why, so the choice is deliberate and reviewable. Versions are the latest
stable releases at the time of writing; see `Cargo.toml` for the exact pins.

| Role | Choice | Rationale |
|---|---|---|
| Configuration layering | `figment` (TOML provider) + `toml` + `serde` | Layers `mortsom.toml` and `mortsom.local.toml`; `env:` indirection is resolved by Mortsom's own pass so every missing secret is reported at once and fails closed. |
| CLI | `clap` (derive) | Standard, subcommand-oriented (`doctor`, `run`, `device`, `relay`, `cleanup`). |
| Async runtime and process management | `tokio` (`process`, `time`, `fs`, `signal`) | One runtime for WebDriver HTTP, relay WebSockets, and child processes (`docker`, `adb`, `emulator`, `avdmanager`, `flutter`). |
| Docker control | `docker` CLI through `tokio::process` behind a `ContainerRuntime` trait | The harness runs a handful of containers per run; the CLI honours the operator's Docker context and credentials, supports `docker compose`, and every invocation can be logged verbatim for reproducibility. `bollard` remains an option if streaming events with backpressure ever becomes necessary. |
| WebDriver protocol | Thin W3C client on `reqwest` (`crates/android-driver`) | `thirtyfour` and `fantoccini` both lack the `accessibility id` locator and Appium endpoints (`mobile:` scripts, `appium/device/*`) that Mortsom uses most, so both would be driven through raw-command escape hatches anyway. A purpose-built ~300-line client is transparent, dependency-light and fully testable with `wiremock`. |
| Nostr and Mostro types | `mostro-core =0.14.5`, `nostr-sdk 0.45.1`, `nostr 0.45.1`, `nwc 0.45.0` | Reuse protocol types (`Status`, `Kind`, event kinds 38383/38385/10002) instead of reimplementing them; the exact pin fixes the whole nostr line. |
| Serialization | `serde`, `serde_json`, `toml` | Artifacts (`summary.json`, `*.jsonl`, `environment.json`, `cleanup.json`) and configuration. |
| Redaction | `mortsom_common::secret::Secret` | Serde-transparent on input, `<redacted>` on `Debug`/`Serialize`, single `expose()` accessor that is greppable; plus a text scrubber for logs and artifacts. |
| Errors | `thiserror` (libraries), `anyhow` (binary edge) | Typed error enums per crate carry the explicit failure category required by spec §10. |
| Logging and reporting | `tracing` + `tracing-subscriber` (env filter, JSON file layer) | Structured run logs per run id; report rendering is pure code over serde types. |
| IDs and time | `uuid`, `chrono` | Run ids and artifact timestamps. |
| Scenario registry | `inventory` | Scenarios self-register with name, tags, required capabilities and timeout; the runner never keeps a hand-maintained list. |
| Tests | `mockall`, `wiremock`, `tempfile`, `assert_cmd`, `cargo-llvm-cov` | Trait fakes for Docker/adb/WebDriver/wallet, a mock Appium server, CLI end-to-end tests and coverage. |

## Crate layout

The workspace mirrors the package tree in spec §12:

```
crates/common              mortsom-common              secrets, run ids, errors, clock, logging
crates/coordinator         mortsom-coordinator         config, runner, budgets, cleanup, reporting, `mortsom` binary
crates/app-contract        mortsom-app-contract        AppAdapter trait, normalized states, capabilities
crates/android-driver      mortsom-android-driver      WebDriver client, AVD/emulator/adb primitives
crates/environment         mortsom-environment         Docker relay, daemon readiness, wallet provisioning
crates/observers           mortsom-observers           UI, notification, relay, wallet, artifact observers
crates/scenario-kit        mortsom-scenario-kit        domain actions, waits, assertions, registry
crates/app-adapters/*      mortsom-adapter-*           one adapter per application generation
scenarios                  mortsom-scenarios           smoke, regression, recovery, adversarial
```

`crates/common` is the only addition to the spec's tree: it holds the pieces
(secret redaction, run identifiers, the failure taxonomy) that every other
crate needs without creating a dependency cycle through the coordinator.

External services (not Rust dependencies): Docker Engine, Android SDK
(`emulator`, `adb`, `avdmanager`, `sdkmanager`), Appium 2 with the
UiAutomator2 driver, Java 17, Flutter, and a locally operated Mostro daemon
connected to a mainnet Lightning node.

## Linux and Web execution (September 2026)

The desktop driver uses `atspi 0.30.0` (latest stable checked with `cargo info`)
for Linux's public accessibility protocol, with its Tokio/zbus transport.
The existing W3C HTTP client also drives browser semantics. Desktop sessions
share the existing UI trait; scenarios remain independent of platform.
Effective-user executable checks use `rustix 1.1.4` with the `fs` feature (latest stable verified via
`cargo info`), avoiding unsafe code and permission-bit approximations.
Ortsom remains an external CLI owning persistent regtest infrastructure.
`mortsom stack up` pulls `mostrop2p/mostro:latest`, resolves its repository digest,
and invokes Ortsom with that immutable image reference. At verification, latest and v0.18.7 resolve to digest
`sha256:bff406d11fc90436c9f7da5186f5976a63ae7ee1d711a6636d36205421cbda31`.

### Client bundle fingerprints

The coordinator uses `sha2` 0.11.0 (latest stable verified with `cargo info sha2` on
2026-09-08) for complete bundle content identity. The versioned
`sha256-mortsom-bundle-v1` manifest sorts UTF-8 relative file paths, then hashes a
fixed domain prefix and each path length, path, content length and file SHA-256.
Lengths use unsigned 64-bit big-endian encoding. Links and special files are
rejected. This records content identity; it does not establish build provenance.
Only explicitly typed public hash fields in run metadata bypass pattern-based
redaction; registered secrets remain redacted, including in those fields.


### Captured clients and offline maintenance

A run copies the complete app bundle into a private temporary directory before
launch. Descriptor-relative file access rejects symlinks and special files; the
captured content must match the initial manifest. Both actors use the captured
copy, which is checked again before startup. Files are read-only, retaining only
necessary executable permissions. `tempfile` 3.27.0 supplies lifetime ownership;
its stable version was verified with `cargo info` on 2026-09-08.

The Web server owns its request tasks and keeps the captured bundle alive until
shutdown has canceled and drained them. Task failures are reported through the
existing `tracing` 0.1.44 dependency (stable version checked on 2026-09-08).
These content and ownership checks do not yet record source/build provenance.

`accounting` and `parity` read only artifact settings. `cleanup` additionally
reads legacy resource settings but routes by the recorded run platform; desktop
recovery remains explicitly unsupported. `publish` resolves configured credential
redaction values without starting Docker or connecting to wallets. Recorded
Android runs retain environment-only credentials; Linux/Web resolve reserved
Ortsom credential paths as the run did. Missing custom credentials or inconsistent
platform metadata prevent publication. Strict trading configuration and spend
caps remain mandatory for `run`.
