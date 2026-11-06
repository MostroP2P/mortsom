# Plan: the `cli` client profile (mostro-cli)

Status: specified; the implementation lands in PRs 5-12 of the stack that
replaces #107 (the results below come from #107's tree, ef807d4).
Base: Mortsom `main` (9dbe627), mostro-cli `main` (6cff9b5).

## Progress

| Phase | State |
|---|---|
| 0 — contract and prerequisites | `spec.md` §6.3 and the related sections, `AGENTS.md`, `docs/cli-contract.md`. mostro-cli: `identity init`/`identity show` and `--invoice -` implemented (specs 009, 010). |
| 1 — driver, adapter, smoke | `crates/cli-driver`, `crates/app-adapters/cli`, `Platform::Cli`, `--profile`, CLI preparation, sessions, evidence (`cli.jsonl`), recovery. Unit and process tests pass. Live: a manual regtest trade with the CLI went pending → waiting-payment → active → fiat-sent → success. |
| 2 — builds and CI | `--ref`, `--source`, `--artifact`, `doctor --profile cli`; workflow `mostro-cli e2e` (Mortsom), by hand with a mostro-cli branch or pull request, and nightly. mostro-cli's own CI does not run Mortsom yet: its pull requests are run from Mortsom by `cli_ref`. |
| 3 — more families | Adapter covers range (`order sync`), cancellations, payment failures, restarts, disputes, rating, bonds. mostro-cli: `order mine`, `--expires-in`, `dispute open`, `order rate`, bond flows and `--reply-timeout` (specs 011–016). |
| 4 — parity | Intentional differences can be scoped to profiles (`profiles = [...]`). Live: `mortsom parity` between the Linux app and the CLI shows parity with no difference on `happy_sell`, `happy_buy` and `cancel_before_taken`. |

### Live results (regtest, mostrod `main` 8111f5e, 2026-11-06)

- Bondless stack, every scenario passed at least once with the CLI: smoke, happy path (range, market
  price, rating), cancellations, expirations (`pending_order_expires` through `--expires-in`), disputes,
  payment failures and the five relaunches. Skipped with their reason: `unauthorized_actions_at_taken`
  (`offered-actions`) and the interruptions.
- Bonded stack (`stack up --bond`): all 15 scenarios of the `bond` family passed with the CLI. Four
  failed on their first attempt, each because mostrod's database was locked (below), and passed when
  rerun.
- Daemon finding: mostrod opens SQLite with sqlx defaults (rollback journal, 5 s busy timeout); under
  load its writes and reads fail with `database is locked` for minutes, with the app as with the CLI.
  On the bonded stack, 56 such errors in 25 minutes hit the bond payout cycle, `AddBondInvoice`,
  `Cancel`, `Orders`, `expire_unpaid_maker_bonds` and `find_inflight_payouts`. A request that fails
  that way is never answered, so the client can only time out. The CLI's reply budget is raised to
  60 s (`--reply-timeout`) so slow answers are not misread as `uncertain`.

## Goal

Run Mortsom's platform-neutral scenarios against
[mostro-cli](https://github.com/MostroP2P/mostro-cli) as well as app v2, by
driving the CLI as a subprocess through its machine interface
(`--output json`). One run then exercises the daemon and the CLI together, and
can be pointed at a mostro-cli pull request to serve as that repository's
end-to-end CI. Scenario bodies do not change: the 49 registered scenarios are
written against `AppAdapter` (`crates/app-contract/src/adapter.rs`); the CLI
gets its own adapter.

## Architecture decisions

1. **`cli` is an application profile with an internal runtime.** Capability
   gating, reports and parity are per profile (`mortsom parity` refuses two runs
   of the same profile). The recovery journal, actor storage, build cache and
   provenance record a `Platform`, so `Platform::Cli` exists for serialization.
   `--platform` keeps accepting only `linux|web`; `run` and `doctor` gain
   `--profile mobile-v2|cli`, which overrides `mobile.profile`. `--platform`
   together with `--profile cli` is an argument error.
2. **A `CliSession` implements `UiSession`** instead of making the UI optional:
   `ActorSession.ui` is required everywhere (scenario kit, bootstrap, farm,
   evidence). The lifecycle maps cleanly: `clear_app` empties the actor's
   private `MCLI_DIR`, `terminate_app`/`activate_app` are process boundaries,
   `page_source` returns the JSON transcript and `diagnostics` the CLI's stderr.
   Element methods answer `UiError::Unsupported`. The adapter reaches the
   session's CLI client through `UiSession::command_line`.
3. **Always a subprocess, never a linked library.** Mortsom is a black-box
   harness: it tests the executable mostro-cli ships, its argument parsing,
   JSON outcome documents, exit codes and identity store included, and
   `--ref`, `--source` or `--artifact` pick that build per run, which a
   library fixed when Mortsom compiles could not do.
4. **Observed state comes from `order info`**, a read-only query
   (`Action::Orders`), so polling never counts as a trade action.
5. **External wallet only.** The CLI has no wallet: Mortsom pays the hold
   invoice from the LND node it holds and issues the buyer invoice there.
6. **Naming.** The profile is `cli`; coordinator modules use a `mostro_cli_`
   prefix so they are not confused with Mortsom's own command line (`cli.rs`).

## Phases

### Phase 0 — contract and prerequisites

- `spec.md`: the CLI as a target client, its isolation (private `MCLI_DIR` and
  `HOME`, emptied environment), builds and PR selection, transcripts instead of
  screenshots, the new capabilities `OfferedActions` and `OrderExpiry`.
- `AGENTS.md`: mostro-cli becomes a system under test, still never a source.
- `docs/cli-contract.md`: the commands, fields and codes Mortsom depends on.
- mostro-cli: `identity init` / `identity show` with JSON, and `--invoice -`
  (the invoice read from stdin so it never appears in process arguments).

### Phase 1 — driver, adapter and the smoke family with a local binary

- `crates/app-contract`: profile `cli`, capabilities `OfferedActions` and
  `OrderExpiry`.
- `crates/ui-driver`: `UiError::Unsupported`, `UiSession::command_line`.
- `crates/desktop-driver`: `Platform::Cli` (serialization only).
- `crates/cli-driver` (new): process boundary with stdin and an emptied
  environment, envelope parser, sanitized transcript, `CliSession`.
- `crates/app-adapters/cli` (new): `CliAdapter`.
- Coordinator: `--profile`, `--artifact`, adapter selection, CLI
  preparation and sessions.
- Exit: `mortsom run --profile cli --artifact <bin> --tag smoke` passes.

### Phase 2 — build from main or a pull request, and CI

- `Upstream` for `MostroP2P/mostro-cli` (`--ref <branch|PR URL>`,
  `--source <dir>`), `cargo build --release --locked`, kept builds.
- `doctor --profile cli`: `mostro-cli schema --output json` must expose
  schema version 1 and the commands the profile requires
  (`docs/cli-contract.md`).
- CI: a Mortsom workflow (manual with a `cli_ref` input, plus nightly).
  mostro-cli has no `pull_request` workflow that runs Mortsom yet, so
  mostro-cli pull requests are run from Mortsom by `cli_ref`.

### Phase 3 — more families

Enabled as mostro-cli gains JSON: market and range orders, cancellations,
expirations (`order new --expires-in`), payment failures, client restarts,
disputes (`dispute open`), rating (`order rate`), own orders (`order mine`)
and anti-abuse bonds.

### Phase 4 — parity

`mortsom parity <mobile-v2 run> <cli run>` on the `parity` tag, with
intentional differences scoped to profiles.

## Requests to mostro-cli

1. Identity: `identity init`, `identity show --output json`.
2. Invoice without process arguments: `--invoice -` reads standard input.
3. Own orders: `order mine --output json`.
4. Per-order expiry in seconds on `order new`.
5. `dispute open` with JSON.
6. `order rate` with JSON.
7. Bonds: declining a bond and reading a slash notice in JSON.
