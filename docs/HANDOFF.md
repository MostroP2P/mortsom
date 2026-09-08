# Resuming the Linux/Web checkpoint

This is the continuation record for the operator-requested token-exhaustion checkpoint.
Read `../spec.md` first, then this file. The spec is normative; this record explains where execution stopped.
All repository content must remain in English; operator-facing conversation is in Spanish.

## Objective and stopping boundary

Finish **happy_sell and happy_buy on Linux and Web: four accepted live cells**, then stop for operator
review. Linux is the default, Web is selectable and Android is the frozen legacy option. Do not run the
other eighteen scenarios or resource benchmarks. The operator explicitly prioritized the detailed spec,
README/quick start and a clean, committed, resumable state.

**Current accepted results: Linux 0/2; Web 0/2.** No funded happy-path trade has passed. Builds, forms,
identity recovery, cancellation diagnostics and unit tests must never be counted as trade acceptance.

## Workspace and repositories

| Project | Path | Working branch |
|---|---|---|
| Mortsom | `/home/negrunch/dev/mortsom` | `feat/v2-desktop-web` |
| App v2 | `/home/negrunch/dev/mobilev2` | `feat/mortsom-linux-accessibility` |
| Ortsom | `/home/negrunch/dev/ortsom` | `feat/public-stack-status` |
| Protocol reference | `/home/negrunch/dev/protocol` | Read-only guidance |
| Daemon reference | `/home/negrunch/dev/mostro` | Read-only guidance |

Ortsom code is committed and pushed through `566a53e`. Draft PR:
https://github.com/MostroP2P/ortsom/pull/35 (not merged). Mortsom and app changes are local commits;
no push/PR has been made for those branches during this checkpoint. Check `git status --short` and
`git log -8 --oneline` in each repository before continuing; never discard unrelated operator changes.

Key completed Mortsom commits:

- `914f758`: bounded native stale-node retries; no replay after business tap invocation.
- `bf2e86e`: protected identity/bundle storage, durable action journal, recovery/cleanup and wallet evidence.
- `a556560`: happy-buy narrative and four injected sequencing/cap/failure regressions.
- `b6bec7c`: native lookup accepts a contract marker after Flutter's tab-position line.
- `fbf42b3`: write recovery evidence before removing protected profiles.
- `dd77f75`: preserve URL characters in dynamic native relay identifiers.
- `cd84b88`: complete live-attempt ledger and invoice-navigation blocker documentation.

App commits before the final form-navigation correction:

- `e83cd8f`: Linux semantics/sidebar, exact manual buyer invoice readouts, final payout mapping/polling/rating.
- `ee6e4bd`: merged TabBar label/action regression and contract documentation, with no runtime change.

The final checkpoint section below records any subsequent source commits and their verification.

## Environment deliberately left available

The persistent `ortsom-regtest` stack is healthy and intentionally remains running. The last read-only
status check found all six services up: bitcoind, relay, mostrod and LND Alice/Bob/Carol. Channels: Alice 1,
Bob 1, Carol 2. Status log: `/tmp/mortsom-handoff-stack-status.log`.

- Observed mostrod version: **0.18.7**, protocol 2.
- Pulled repository digest: `sha256:bff406d11fc90436c9f7da5186f5976a63ae7ee1d711a6636d36205421cbda31`.
- Running image ID: `sha256:277bce332c70a76bfc7ef04598bb6fbdfad6939a805031518efe9686129d3e36`.
- Current public daemon identity: `deccad95b70e7de5befa7de72718510c5765ed0b05649fcb196be56918e8382b`.
- Relay: `ws://127.0.0.1:7000`; LND endpoints: Alice 10001, Bob 10002, Carol 10003.
- Credentials stay in the stack's protected files, resolved through public connection references. Do not
  print or copy their contents into docs, config, logs or artifacts.

**Use `stack status` first, not `stack up`.** Stack up pulls latest and may regenerate identity; resetting
requires both clients to be rebuilt and is not evidence of cancellation/refund. No reset is needed to
continue from this checkpoint.

The three recent recovery journals were audited: all are `closed=true`, `resolution_verified=true`, with
actor/profile and captured-bundle directories removed. Their order cleanup was verified; no invoice or
payment was issued. No Mortsom client, Xvfb, ChromeDriver or scenario process remained at the audit. Existing
operator browsers and unrelated processes must not be killed. The shared daemon remains by design.

Disk at the audit: approximately 34 GiB free on `/home` (96% used). App/Rust builds are large; watch space
before coverage builds. Do not delete artifacts or private recovery state just to gain space.

## What has passed

G0/G1/G2 are accepted in the spec. G3 (live trades) is not.

- Full workspace verification at G2: **604 tests passed, zero failed, one existing ignored integration
  test**, plus formatting, strict workspace clippy and coordinator binary build.
- Native parser follow-up: **42 desktop tests passed**, strict workspace clippy and rebuilt coordinator.
- Recovery evidence follow-up: three focused tests cover six failed artifact leaves, clean removal and a
  final post-resolution summary-write failure. Strict coordinator clippy passed.
- App at `e83cd8f`: 338 Flutter tests passed (two expected unarmed skips), 412 Rust tests passed (18 existing
  ignored tests), Flutter analysis and Rust strict clippy passed.
- Armed Linux accessibility after `ee6e4bd`: five tests passed, including actual merged TabBar labels and
  semantic tap actions. This confirms why native lookup must accept the marker at a line boundary.
- Real Linux recovery probe `20260908T154215Z-53d3093b0ef8`: both original identities survived new handles.
- Real Web recovery probe `20260908T154752Z-b4c8168f49a0`: both identities and exact Web origin survived new
  handles/server restart. Both probes stopped all owned resources and removed profiles; zero orders/payments.

Useful logs:

```text
/tmp/mortsom-checkpoint-final-tests.log
/tmp/mortsom-checkpoint-final-clippy.log
/tmp/mortsom-checkpoint-final-build.log
/tmp/mortsom-native-relay-id-{red,green,clippy,build}.log
/tmp/mortsom-recovery-evidence-order-{red,green,clippy}.log
/tmp/mortsom-linux-recovery-smoke.log
/tmp/mortsom-web-recovery-smoke.log
/tmp/mortsom-app-final-flutter-test.log
/tmp/mortsom-app-final-flutter-analyze.log
/tmp/mortsom-app-full-rust-test.log
/tmp/mortsom-app-full-rust-clippy.log
/tmp/mortsom-linux-tab-semantics-green.log
```

Logs under `/tmp` are local evidence and may not survive a host cleanup. Persistent run evidence remains
under gitignored `artifacts/`; recovery probe result JSON is under
`$HOME/.local/state/mortsom/recovery-probes/evidence/<run>/result.json`. Never commit private storage or
artifacts. Coverage >=80% and final delivery audit are still pending.

## Live failures already diagnosed

Full detail is in spec section 18.14 and `docs/ortsom-ui-parity.md`.

| Run | Result |
|---|---|
| `20260908T160318Z-e617fd03c704` | Created sell `911fcef1-b3eb-497b-8018-3f3c2d97071d`; Bob could not locate merged native book-tab label. UI cancellation and full cleanup passed. |
| `20260908T161029Z-86b51b35d10b` | Parser whitelist rejected relay IDs containing `ws://`; bootstrap failed before a scenario. Explicit no-intent cleanup removed retained profiles. |
| `20260908T161350Z-259115af1b1c` | Created/took sell `11284250-f982-43ba-8c2b-33aadbede145`; Bob landed on the buyer invoice form, while take adapter waited for order.status. UI cancellation and full cleanup passed. |

For the latest failure, Bob's public form contained `invoice.order_id` matching the order and
`invoice.amount=995` for the 1000-sat trade. The form was empty: no invoice was issued or submitted and no
hold was paid. Logs are `/tmp/mortsom-checkpoint-happy-sell-linux{,-2,-3}.log`.

The app's normal take routes are important:

- Taking a sell (Bob buyer) navigates directly to the buyer invoice form.
- Taking a buy (Alice seller) navigates directly to the seller hold-payment form.
- A form-derived Taken observation must match its exact public order ID. A foreign form cannot authorize
  a payment or prove the requested order's state.
- `pay.cancel` **cancels the order**; it is not a navigation-back shortcut.

The final source correction is described below. It still requires rebuilt clients and live verification.

## Implementation invariants to preserve

- UI-only business actions. No direct protocol order mutations, private app methods or app database access.
- Rust harness; app-owned Dart/Rust contract changes stay in the app repository.
- Keep `mostro-core =0.14.5` and its Nostr dependency line unchanged.
- Desktop checkpoint uses both external LND wallets. NWC desktop recovery remains deferred.
- Write durable submission/payment intent before the potentially irreversible UI/RPC operation. Never
  blindly resubmit an order after a missing confirmation or retry an uncertain payment.
- Actual seller invoice amount plus fee ceiling is reserved before payment. Pre-call failures release
  unused preparation; ambiguous invoked payments remain InFlight. Pending hold handles survive await
  timeout/cancellation until terminal accounting succeeds.
- Escrow-settled/payout-pending is not completed. Require both clients completed, correlated relay state,
  exact hold/payout outcomes and one actual settled ledger record per hash. No optional-hash pass.
- Cleanup restores original public identities, exact bundle and stack; Web restores its exact origin.
  Missing/tampered storage or changed stack refuses actions. Failed evidence before resolution retains
  profiles. Final reporting after verified deletion can fail; the durable closed manifest is authoritative.
- SIGKILL leaves active-runtime markers and fails closed. Automatic orphan reconciliation is unsupported;
  never remove a marker simply to bypass the refusal.
- Keep the spec, README and parity ledger current. Commit completed tasks after review, fmt and strict
  workspace clippy; run meaningful tests. No new live case outside the four-cell scope.

## Resume commands

From `/home/negrunch/dev/mortsom`, inspect source and stack first:

```sh
git status --short
git log -8 --oneline
./target/debug/mortsom stack status
```

Read the final checkpoint notes below before choosing the next action. If the source correction has not
been live-validated, build both app targets **sequentially** against the unchanged stack:

```sh
cargo build -p mortsom-coordinator --bin mortsom
cargo run -p mortsom-coordinator --example prepare_client -- linux
cargo run -p mortsom-coordinator --example prepare_client -- web
```

The example uses `config/desktop.example.toml` and `../ortsom`. Do not build both targets concurrently:
app bridge generation writes shared source files. These builds create no order/payment.

Then run one live scenario at a time, beginning with Linux happy_sell:

```sh
./target/debug/mortsom run happy_sell --platform linux \
  --app-artifact /home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro
./target/debug/mortsom run happy_buy --platform linux \
  --app-artifact /home/negrunch/dev/mobilev2/build/linux/x64/release/bundle/mostro
```

For Web on this host, the known working executables are:

```sh
export MORTSOM_CHROMEDRIVER=/snap/chromium/current/usr/lib/chromium-browser/chromedriver
export MORTSOM_BROWSER=/snap/chromium/current/usr/lib/chromium-browser/chrome
./target/debug/mortsom run happy_sell --platform web \
  --app-artifact /home/negrunch/dev/mobilev2/build/web
./target/debug/mortsom run happy_buy --platform web \
  --app-artifact /home/negrunch/dev/mobilev2/build/web
```

Check each exit/result/cleanup before starting the next. If it fails, fix the observed error and recover
that original run with `mortsom cleanup <run-id>` when needed. Do not use an environment reset to claim a
payment refunded. Keep failed evidence, record the failure in the spec, and use fresh actors for the retry.
Once the four cells pass, complete remaining checks/coverage and reviews, update all documents and stop.

## Final checkpoint — pause before rebuilding clients

The operator reported only 3% of the weekly allowance remaining. Work is paused at a committed source
checkpoint; do not infer a completed four-case goal.

- Mortsom source commit **`e846715`**: take recognizes exact order-bound invoice/payment forms, observes
  their normal Taken state, rejects foreign forms and reuses the matching seller invoice without extra taps.
- App source commit **`0ed7efb`**: visible `pay.order_id` and ordinary `appbar.back` on invoice routes.
- Independent coordinated review passed. Full v2 adapter suite: **44 passed** (six route regressions).
  Focused app invoice suite: **5 passed**, targeted Flutter analysis clean; strict workspace clippy clean.
  Logs: `/tmp/mortsom-take-adapter-full.log`, `/tmp/mortsom-take-forms-clippy.log`,
  `/tmp/mortsom-invoice-route-widget-tests.log`, `/tmp/mortsom-invoice-route-analyze.log`.
- **Both existing app bundles are stale.** They predate `0ed7efb`; rebuild Linux and Web before any new live
  trade. The existing coordinator binary can also predate `e846715`; rebuild it first as shown above.
- No fourth live attempt was started. Latest live result remains the third failed Linux sell, whose cleanup
  passed. There is no pending payment/order/profile from these three runs.
- README now contains the complete current usage guide and quick start. Historical Android instructions
  live in `docs/legacy-android.md`; AGENTS.md points resumed work here. The spec and parity ledger retain
  failures and the unchanged Linux0/2, Web0/2 acceptance.

The immediate next action is **rebuild coordinator, rebuild Linux and Web sequentially, then retry Linux
happy_sell**, following the commands above. Do not repeat completed diagnosis or reset the healthy stack.
Final coverage >=80%, successful funded evidence on all four cells and delivery audit remain outstanding.

Final pause verification completed: **613 workspace tests passed, zero failed, one existing ignored test**;
formatting and strict workspace/all-targets clippy passed. Logs:
`/tmp/mortsom-handoff-final-tests.log` and `/tmp/mortsom-handoff-final-clippy.log`.
No scenario, test, app client, Xvfb or ChromeDriver process remained after this check. The persistent
regtest stack remains running intentionally. Coverage was not run at this pause checkpoint.
