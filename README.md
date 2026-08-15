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

## Safety

Mortsom moves real sats on mainnet through the wallets the operator provides.
It refuses value-moving scenarios unless `run.allow_mainnet = true` **and**
`MORTSOM_MAINNET_CONFIRM=I_ACCEPT_MAINNET_SPEND` are both present, and it
never runs without per-order and per-run spend caps.
