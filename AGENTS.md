# AGENTS.md

Guidance for AI agents and human contributors working on Mortsom.

## Project

Mortsom is a black-box, end-to-end test harness for the Mostro **mobile
application**. It drives two installations of the app (Alice and Bob) on
independent Android devices, like real users, against a locally operated
Mostro daemon and a run-owned Nostr relay. Read `spec.md` before touching
anything — it is the design document this codebase implements, and every
phase in §17 has an exit gate that must pass before the next phase starts.

Ortsom (`../ortsom`) is the sibling harness for the daemon and protocol. The
two projects share concepts and selected artifacts, never source code or a
runner.

## Language policy

**Everything in this repository is written in English**: code, comments,
identifiers, documentation, commit messages, issues, pull requests, log
messages, error strings, and artifacts. No exceptions.

## Source of truth

- `spec.md` is normative for Mortsom's behaviour. When code and spec disagree,
  fix the code or change the spec explicitly in the same change.
- The Mostro **protocol specification** (https://mostro.network/protocol/,
  source https://github.com/MostroP2P/protocol) is normative for everything on
  the wire. **Protocol v1 (gift-wrap transport) is obsolete**; never implement
  or accommodate it.
- The mobile app repository (`../mobile`, later `../mobilev2`) owns the
  accessibility identifiers and bootstrap hooks documented in its
  `docs/automation-contract.md`. Mortsom owns automation, scenarios,
  environment orchestration and reports. A change to a semantic identifier,
  a visible business state, or an external-wallet callback is a contract
  change and requires coordinated review.
- Reference implementations (`../mostro-cli`, `../mostrix`, `../ortsom`) are
  guidance, not a source. Do not copy code from them.

## Stack

- **Rust only.** The coordinator, configuration, Docker and emulator
  lifecycle, WebDriver client, observers, scenario kit, adapters, reporting
  and CLI are Rust crates. Appium and UiAutomator2 run only as external
  services reached through the W3C WebDriver protocol. No TypeScript or Node
  application code belongs in this repository.
- The crate selection and its rationale live in `docs/stack.md`.

## Dependencies

- Always use the **latest stable versions** of dependencies. Before adding or
  bumping a crate, check crates.io (`cargo info <crate>`) for the current
  stable release. Do not add a crate at an older version because a reference
  project uses it.
- Exception: `mostro-core` is pinned exactly (`=0.14.5`), which transitively
  fixes `nostr`/`nostr-sdk` at 0.45.1 and `nwc` at 0.45.0. Every Nostr-touching
  crate must stay on that line until the pin moves; move the pin in its own
  commit.
- Reuse `mostro-core` types (statuses, kinds, event constants); do not
  reimplement the protocol.

## Code style

- Rust 2024 edition, MSRV in the workspace manifest. `cargo fmt --all` and
  `cargo clippy --workspace --all-targets -- -D warnings` must pass before
  every commit.
- No fixed sleeps in scenario code. Every wait is a bounded predicate over UI
  state, notification, relay event, wallet callback, or an explicit
  negative-observation interval.
- Push I/O to the edges. Configuration resolution, redaction, event matching,
  state normalization, budgeting and report rendering are pure and unit
  tested.
- A trait boundary at every external dependency (Docker, adb/emulator,
  WebDriver, relay transport, wallet, clock, environment variables) so tests
  inject fakes.
- Small focused files (200–400 lines typical, 800 max), functions under 50
  lines, no nesting deeper than four levels.
- Handle errors explicitly with typed error enums; never swallow them.
  Failure categories (product, automation contract, infrastructure, relay,
  wallet, environment misconfiguration) are explicit values, never inferred
  from message text.

## Testing

- TDD: write the failing test first, implement, watch it pass, refactor.
- Minimum coverage is 80% lines via `cargo llvm-cov`; the CI floor only
  ratchets upward. Scenario bodies are the tests, not code under test.
- Unit tests live next to the code (`#[cfg(test)] mod tests`); integration
  tests live in each crate's `tests/`; CLI tests use `assert_cmd`.
- Test names describe behaviour (`refuses_mainnet_without_confirmation`).

## Safety

- Mortsom moves real sats on **mainnet** through the wallets configured by
  the operator. Never weaken spend caps, per-order caps, accounting, the
  mainnet guard, or cleanup. A missing cap is an error, never an unlimited
  default.
- The lane (`local`, `ci`, `canary`) is named in configuration and in every
  report; nothing infers mainnet.
- **No secrets in the repository, ever.** Mnemonics, private keys, NWC URIs,
  macaroons and tokens are referenced from configuration via `env:`
  indirection and live only in the environment. `mortsom.toml` and
  `*.local.toml` are gitignored. Secrets are wrapped in `Secret` the moment
  they are read and never appear in logs, artifacts, screenshots or reports.
- The suite observes only what a user or the test environment can observe:
  accessibility tree, pixels, process state, notifications, run-owned relay
  events, run-owned wallet payments and infrastructure readiness. It never
  reads the app database, invokes private methods, or mutates orders outside
  the UI.
- `mortsom doctor` performs no payment and creates no order.

## Git

- Conventional commits: `<type>: <description>` with types
  `feat, fix, refactor, docs, test, chore, perf, ci`.
- One commit per completed spec task, after its code review.
- Never commit `mortsom.toml`, `*.local.toml`, `.env*` or anything under
  `artifacts/`.
