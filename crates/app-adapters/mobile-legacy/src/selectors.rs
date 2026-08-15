//! Semantic identifiers of the classic app, mirrored from
//! `mobile/lib/core/automation/automation_ids.dart`. The contract test
//! `selectors_match_the_app_contract` proves the mirror is exact whenever
//! the app checkout is available.

/// Identifiers (`Semantics.identifier`, Android `resource-id`).
pub struct Ids;

#[allow(missing_docs)]
impl Ids {
    pub const ENV_MARKER: &'static str = "env.marker";
    pub const APPBAR_DRAWER: &'static str = "appbar.drawer";
    pub const APPBAR_BACK: &'static str = "appbar.back";
    pub const NAV_ORDER_BOOK: &'static str = "nav.order_book";
    pub const NAV_TRADES: &'static str = "nav.trades";
    pub const NAV_CHAT: &'static str = "nav.chat";
    pub const DRAWER_ACCOUNT: &'static str = "drawer.account";
    pub const DRAWER_SETTINGS: &'static str = "drawer.settings";
    pub const DRAWER_ABOUT: &'static str = "drawer.about";
    pub const ONBOARDING_BACK: &'static str = "onboarding.walkthrough.back";
    pub const ONBOARDING_SKIP: &'static str = "onboarding.walkthrough.skip";
    pub const ONBOARDING_NEXT: &'static str = "onboarding.walkthrough.next";
    pub const ONBOARDING_DONE: &'static str = "onboarding.walkthrough.done";
    pub const COMMUNITY_CUSTOM_NODE: &'static str = "onboarding.community.custom_node";
    pub const COMMUNITY_DONE: &'static str = "onboarding.community.done";
    pub const COMMUNITY_SKIP: &'static str = "onboarding.community.skip";
    pub const KEYS_GENERATE: &'static str = "keys.generate";
    pub const KEYS_GENERATE_CONFIRM: &'static str = "keys.generate.confirm";
    pub const KEYS_IMPORT: &'static str = "keys.import";
    pub const KEYS_IMPORT_MNEMONIC: &'static str = "keys.import.mnemonic";
    pub const KEYS_IMPORT_CONFIRM: &'static str = "keys.import.confirm";
    pub const KEYS_IMPORT_CANCEL: &'static str = "keys.import.cancel";
    pub const KEYS_SEED_REVEAL: &'static str = "keys.seed.reveal";
    pub const KEYS_SEED_TEXT: &'static str = "keys.seed.text";
    pub const KEYS_PUBLIC_KEY: &'static str = "keys.public_key";
    pub const SETTINGS_MOSTRO_NODE: &'static str = "settings.mostro_node";
    pub const SETTINGS_MOSTRO_NODE_PUBKEY: &'static str = "settings.mostro_node.pubkey";
    pub const SETTINGS_WALLET: &'static str = "settings.wallet";
    pub const SETTINGS_RELAYS_ADD: &'static str = "settings.relays.add";
    pub const SETTINGS_RELAYS_ADD_URL: &'static str = "settings.relays.add.url";
    pub const SETTINGS_RELAYS_ADD_CONFIRM: &'static str = "settings.relays.add.confirm";
    pub const SETTINGS_RELAYS_ADD_CANCEL: &'static str = "settings.relays.add.cancel";
    pub const SETTINGS_RELAY_DELETE: &'static str = "settings.relays.item.delete";
    pub const NODE_ADD_CUSTOM: &'static str = "node.add_custom";
    pub const NODE_CUSTOM_PUBKEY: &'static str = "node.custom.pubkey";
    pub const NODE_CUSTOM_NAME: &'static str = "node.custom.name";
    pub const NODE_CUSTOM_CONFIRM: &'static str = "node.custom.confirm";
    pub const NODE_CUSTOM_CANCEL: &'static str = "node.custom.cancel";
    pub const WALLET_NWC_URI: &'static str = "wallet.nwc.uri";
    pub const WALLET_NWC_CONNECT: &'static str = "wallet.nwc.connect";
    pub const WALLET_CONNECTION: &'static str = "wallet.connection";
    pub const WALLET_SETTINGS_CONNECT: &'static str = "wallet.settings.connect";
    pub const WALLET_SETTINGS_DISCONNECT: &'static str = "wallet.settings.disconnect";
    pub const ORDER_ADD_FAB: &'static str = "order.add.fab";
    pub const ORDER_ADD_BUY: &'static str = "order.add.buy";
    pub const ORDER_ADD_SELL: &'static str = "order.add.sell";
    pub const ORDER_CREATE_CURRENCY: &'static str = "order.create.currency";
    pub const ORDER_CREATE_FIAT_AMOUNT: &'static str = "order.create.fiat_amount";
    pub const ORDER_CREATE_FIAT_AMOUNT_MAX: &'static str = "order.create.fiat_amount_max";
    pub const ORDER_CREATE_PAYMENT_METHOD: &'static str = "order.create.payment_method";
    pub const ORDER_CREATE_PRICE_TYPE: &'static str = "order.create.price_type";
    pub const ORDER_CREATE_SATS_AMOUNT: &'static str = "order.create.sats_amount";
    pub const ORDER_CREATE_SUBMIT: &'static str = "order.create.submit";
    pub const ORDER_CREATE_CANCEL: &'static str = "order.create.cancel";
    pub const ORDER_CONFIRM_HOME: &'static str = "order.confirm.home";
    pub const ORDER_TAKE_CONFIRM: &'static str = "order.take.confirm";
    pub const ORDER_TAKE_CLOSE: &'static str = "order.take.close";
    pub const ORDER_TAKE_AMOUNT: &'static str = "order.take.amount";
    pub const ORDER_TAKE_AMOUNT_CONFIRM: &'static str = "order.take.amount.confirm";
    pub const ORDER_ID: &'static str = "order.id";
    pub const ORDER_STATUS: &'static str = "order.status";
    pub const TRADES_ITEM_STATUS: &'static str = "trades.item.status";
    pub const TRADE_FIAT_SENT: &'static str = "trade.fiatSent";
    pub const TRADE_RELEASE: &'static str = "trade.release";
    pub const TRADE_RELEASE_CONFIRM: &'static str = "trade.release.confirm";
    pub const TRADE_CANCEL: &'static str = "trade.cancel";
    pub const TRADE_CANCEL_CONFIRM: &'static str = "trade.cancel.confirm";
    pub const TRADE_DISPUTE: &'static str = "trade.dispute";
    pub const TRADE_DISPUTE_CONFIRM: &'static str = "trade.dispute.confirm";
    pub const TRADE_PAY_INVOICE: &'static str = "trade.payInvoice";
    pub const TRADE_ADD_INVOICE: &'static str = "trade.addInvoice";
    pub const TRADE_TAKE_SELL: &'static str = "trade.takeSell";
    pub const TRADE_TAKE_BUY: &'static str = "trade.takeBuy";
    pub const INVOICE_TEXT: &'static str = "invoice.text";
    pub const INVOICE_SUBMIT: &'static str = "invoice.submit";
    pub const INVOICE_CANCEL: &'static str = "invoice.cancel";
    pub const PAY_NWC: &'static str = "pay.nwc";
    pub const PAY_CANCEL: &'static str = "pay.cancel";

    /// Every static identifier, for contract tests.
    pub const ALL: &'static [&'static str] = &[
        Self::ENV_MARKER,
        Self::APPBAR_DRAWER,
        Self::APPBAR_BACK,
        Self::NAV_ORDER_BOOK,
        Self::NAV_TRADES,
        Self::NAV_CHAT,
        Self::DRAWER_ACCOUNT,
        Self::DRAWER_SETTINGS,
        Self::DRAWER_ABOUT,
        Self::ONBOARDING_BACK,
        Self::ONBOARDING_SKIP,
        Self::ONBOARDING_NEXT,
        Self::ONBOARDING_DONE,
        Self::COMMUNITY_CUSTOM_NODE,
        Self::COMMUNITY_DONE,
        Self::COMMUNITY_SKIP,
        Self::KEYS_GENERATE,
        Self::KEYS_GENERATE_CONFIRM,
        Self::KEYS_IMPORT,
        Self::KEYS_IMPORT_MNEMONIC,
        Self::KEYS_IMPORT_CONFIRM,
        Self::KEYS_IMPORT_CANCEL,
        Self::KEYS_SEED_REVEAL,
        Self::KEYS_SEED_TEXT,
        Self::KEYS_PUBLIC_KEY,
        Self::SETTINGS_MOSTRO_NODE,
        Self::SETTINGS_MOSTRO_NODE_PUBKEY,
        Self::SETTINGS_WALLET,
        Self::SETTINGS_RELAYS_ADD,
        Self::SETTINGS_RELAYS_ADD_URL,
        Self::SETTINGS_RELAYS_ADD_CONFIRM,
        Self::SETTINGS_RELAYS_ADD_CANCEL,
        Self::SETTINGS_RELAY_DELETE,
        Self::NODE_ADD_CUSTOM,
        Self::NODE_CUSTOM_PUBKEY,
        Self::NODE_CUSTOM_NAME,
        Self::NODE_CUSTOM_CONFIRM,
        Self::NODE_CUSTOM_CANCEL,
        Self::WALLET_NWC_URI,
        Self::WALLET_NWC_CONNECT,
        Self::WALLET_CONNECTION,
        Self::WALLET_SETTINGS_CONNECT,
        Self::WALLET_SETTINGS_DISCONNECT,
        Self::ORDER_ADD_FAB,
        Self::ORDER_ADD_BUY,
        Self::ORDER_ADD_SELL,
        Self::ORDER_CREATE_CURRENCY,
        Self::ORDER_CREATE_FIAT_AMOUNT,
        Self::ORDER_CREATE_FIAT_AMOUNT_MAX,
        Self::ORDER_CREATE_PAYMENT_METHOD,
        Self::ORDER_CREATE_PRICE_TYPE,
        Self::ORDER_CREATE_SATS_AMOUNT,
        Self::ORDER_CREATE_SUBMIT,
        Self::ORDER_CREATE_CANCEL,
        Self::ORDER_CONFIRM_HOME,
        Self::ORDER_TAKE_CONFIRM,
        Self::ORDER_TAKE_CLOSE,
        Self::ORDER_TAKE_AMOUNT,
        Self::ORDER_TAKE_AMOUNT_CONFIRM,
        Self::ORDER_ID,
        Self::ORDER_STATUS,
        Self::TRADES_ITEM_STATUS,
        Self::TRADE_FIAT_SENT,
        Self::TRADE_RELEASE,
        Self::TRADE_RELEASE_CONFIRM,
        Self::TRADE_CANCEL,
        Self::TRADE_CANCEL_CONFIRM,
        Self::TRADE_DISPUTE,
        Self::TRADE_DISPUTE_CONFIRM,
        Self::TRADE_PAY_INVOICE,
        Self::TRADE_ADD_INVOICE,
        Self::TRADE_TAKE_SELL,
        Self::TRADE_TAKE_BUY,
        Self::INVOICE_TEXT,
        Self::INVOICE_SUBMIT,
        Self::INVOICE_CANCEL,
        Self::PAY_NWC,
        Self::PAY_CANCEL,
    ];

    /// Community/node card for a pubkey during onboarding.
    pub fn community_card(pubkey: &str) -> String {
        format!("onboarding.community.card.{pubkey}")
    }
    /// Node row in the node selector.
    pub fn node_item(pubkey: &str) -> String {
        format!("node.item.{pubkey}")
    }
    /// Relay row in settings.
    pub fn relay_item(url: &str) -> String {
        format!("settings.relays.item.{url}")
    }
    /// Order book row.
    pub fn order_book_item(order_id: &str) -> String {
        format!("order.book.item.{order_id}")
    }
    /// Trades list row.
    pub fn trades_item(order_id: &str) -> String {
        format!("trades.item.{order_id}")
    }
    /// Currency option in the picker.
    pub fn currency_option(code: &str) -> String {
        format!("order.create.currency.{code}")
    }
    /// Trade action button by protocol action name.
    pub fn trade_action(action: &str) -> String {
        format!("trade.{action}")
    }
}

/// Path of the app's identifier registry, relative to the app checkout.
pub const APP_IDS_FILE: &str = "lib/core/automation/automation_ids.dart";

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    #[test]
    fn identifiers_are_unique_and_namespaced() {
        let mut seen = std::collections::BTreeSet::new();
        for id in Ids::ALL {
            assert!(seen.insert(*id), "duplicate {id}");
            assert!(id.contains('.'), "{id} is not namespaced");
        }
    }

    #[test]
    fn dynamic_helpers_keep_prefixes() {
        assert_eq!(Ids::community_card("ab"), "onboarding.community.card.ab");
        assert_eq!(Ids::relay_item("ws://x"), "settings.relays.item.ws://x");
        assert_eq!(Ids::trade_action("fiatSent"), Ids::TRADE_FIAT_SENT);
        assert_eq!(Ids::node_item("p"), "node.item.p");
        assert_eq!(Ids::order_book_item("o"), "order.book.item.o");
        assert_eq!(Ids::trades_item("o"), "trades.item.o");
        assert_eq!(Ids::currency_option("USD"), "order.create.currency.USD");
    }

    /// Guard: the seed phrase is sensitive and never appears in evidence.
    /// No screen object may reveal or read it; only this registry names it.
    #[test]
    fn seed_phrase_identifiers_are_never_used_by_screen_objects() {
        let src = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        for entry in walk(&src) {
            if entry.ends_with("selectors.rs") {
                continue;
            }
            let text = std::fs::read_to_string(&entry).unwrap();
            assert!(
                !text.contains("KEYS_SEED_REVEAL"),
                "{} reveals the seed",
                entry.display()
            );
            assert!(
                !text.contains("KEYS_SEED_TEXT"),
                "{} reads the seed",
                entry.display()
            );
        }
    }

    fn walk(dir: &std::path::Path) -> Vec<std::path::PathBuf> {
        let mut out = Vec::new();
        for entry in std::fs::read_dir(dir).unwrap().flatten() {
            let path = entry.path();
            if path.is_dir() {
                out.extend(walk(&path));
            } else if path.extension().is_some_and(|e| e == "rs") {
                out.push(path);
            }
        }
        out
    }

    /// Contract test: every identifier the adapter uses exists literally in
    /// the app's registry. Runs only when the checkout is present.
    #[test]
    fn selectors_match_the_app_contract() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../../../mobile")
            .join(APP_IDS_FILE);
        let Ok(dart) = std::fs::read_to_string(&path) else {
            eprintln!("skipping: {} not found", path.display());
            return;
        };
        for id in Ids::ALL {
            assert!(
                dart.contains(&format!("'{id}'")),
                "{id} missing from {}",
                path.display()
            );
        }
        for prefix in [
            "onboarding.community.card.$",
            "node.item.$",
            "settings.relays.item.$",
            "order.book.item.$",
            "trades.item.$",
            "order.create.currency.$",
            "trade.$",
        ] {
            assert!(dart.contains(prefix), "dynamic prefix {prefix} missing");
        }
    }
}
