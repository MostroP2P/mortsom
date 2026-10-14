//! Semantic identifiers of the v2 app (`Semantics.identifier`), as
//! specified in `docs/history/mobile-v2-inventory.md`. Where a
//! screen exists in both applications the identifier is the same so the
//! contract stays one vocabulary; v2-only widgets have v2-only ids. The
//! contract test `selectors_match_the_v2_app_contract` proves the mirror
//! exact whenever the v2 checkout carries `automation_ids.dart`.

/// Identifiers.
pub struct Ids;

#[allow(missing_docs)]
impl Ids {
    pub const ENV_MARKER: &'static str = "env.marker";
    pub const APPBAR_DRAWER: &'static str = "appbar.drawer";
    pub const APPBAR_BACK: &'static str = "appbar.back";
    pub const NAV_ORDER_BOOK: &'static str = "nav.order_book";
    pub const NAV_TRADES: &'static str = "nav.trades";
    // My Trades loaded with nothing to show, and failed to load. With
    // neither and no row, the list is still loading.
    pub const TRADES_EMPTY: &'static str = "trades.empty";
    pub const TRADES_ERROR: &'static str = "trades.error";
    pub const NAV_CHAT: &'static str = "nav.chat";
    pub const DRAWER_ACCOUNT: &'static str = "drawer.account";
    pub const DRAWER_SETTINGS: &'static str = "drawer.settings";
    pub const DRAWER_ABOUT: &'static str = "drawer.about";
    // v2 walkthrough has three slides; the first run then asks for a node.
    pub const WALKTHROUGH_SKIP: &'static str = "onboarding.walkthrough.skip";
    pub const WALKTHROUGH_NEXT: &'static str = "onboarding.walkthrough.next";
    pub const WALKTHROUGH_DONE: &'static str = "onboarding.walkthrough.done";
    pub const COMMUNITY_DONE: &'static str = "onboarding.community.done";
    // Key management (account screen).
    pub const KEYS_GENERATE: &'static str = "keys.generate";
    pub const KEYS_GENERATE_CONFIRM: &'static str = "keys.generate.confirm";
    pub const KEYS_IMPORT: &'static str = "keys.import";
    pub const KEYS_PUBLIC_KEY: &'static str = "keys.public_key";
    // Settings: node selector dialog, relays route, wallet route.
    pub const SETTINGS_MOSTRO_NODE: &'static str = "settings.mostro_node";
    pub const SETTINGS_MOSTRO_NODE_PUBKEY: &'static str = "settings.mostro_node.pubkey";
    pub const NODE_CUSTOM_PUBKEY: &'static str = "node.custom.pubkey";
    pub const NODE_CUSTOM_CONFIRM: &'static str = "node.custom.confirm";
    pub const NODE_CUSTOM_CANCEL: &'static str = "node.custom.cancel";
    pub const SETTINGS_RELAYS: &'static str = "settings.relays";
    pub const SETTINGS_RELAYS_ADD: &'static str = "settings.relays.add";
    pub const SETTINGS_RELAYS_ADD_URL: &'static str = "settings.relays.add.url";
    pub const SETTINGS_RELAYS_ADD_CONFIRM: &'static str = "settings.relays.add.confirm";
    pub const SETTINGS_RELAYS_ADD_CANCEL: &'static str = "settings.relays.add.cancel";
    pub const SETTINGS_WALLET: &'static str = "settings.wallet";
    pub const WALLET_SETTINGS_DISCONNECT: &'static str = "wallet.settings.disconnect";
    pub const WALLET_NWC_URI: &'static str = "wallet.nwc.uri";
    pub const WALLET_NWC_PASTE: &'static str = "wallet.nwc.paste";
    pub const WALLET_NWC_CONNECT: &'static str = "wallet.nwc.connect";
    pub const WALLET_CONNECTION: &'static str = "wallet.connection";
    // Order book and creation.
    pub const ORDER_BOOK_TAB_BUY: &'static str = "order.book.tab.buy";
    pub const ORDER_BOOK_TAB_SELL: &'static str = "order.book.tab.sell";
    pub const ORDER_ADD_FAB: &'static str = "order.add.fab";
    pub const ORDER_ADD_BUY: &'static str = "order.add.buy";
    pub const ORDER_ADD_SELL: &'static str = "order.add.sell";
    pub const ORDER_CREATE_CURRENCY: &'static str = "order.create.currency";
    pub const ORDER_CREATE_CURRENCY_SEARCH: &'static str = "order.create.currency.search";
    pub const ORDER_CREATE_FIAT_AMOUNT: &'static str = "order.create.fiat_amount";
    // Range orders: the `Single | Range` control swaps the single amount
    // for a min/max pair. The control names the whole; a driver taps the
    // segment.
    pub const ORDER_CREATE_RANGE: &'static str = "order.create.range";
    pub const ORDER_CREATE_AMOUNT_RANGE: &'static str = "order.create.amount.range";
    pub const ORDER_CREATE_FIAT_MIN: &'static str = "order.create.fiat_min";
    pub const ORDER_CREATE_FIAT_MAX: &'static str = "order.create.fiat_max";
    // Payment methods are chosen on their own screen, opened from the form.
    // A free-text method goes through a sheet: its field, then `Add`; the
    // screen's confirm is the only way the choice reaches the form.
    pub const ORDER_CREATE_PAYMENT_METHOD_ADD: &'static str = "order.create.payment_method.add";
    pub const ORDER_CREATE_PAYMENT_METHOD_CUSTOM_OPEN: &'static str =
        "order.create.payment_method.custom_open";
    pub const ORDER_CREATE_PAYMENT_METHOD: &'static str = "order.create.payment_method";
    pub const ORDER_CREATE_PAYMENT_METHOD_CUSTOM_ADD: &'static str =
        "order.create.payment_method.custom_add";
    pub const ORDER_CREATE_PAYMENT_METHODS_CONFIRM: &'static str =
        "order.create.payment_methods.confirm";
    // `Market | Fixed` control, as a whole and per segment.
    pub const ORDER_CREATE_PRICE_TYPE: &'static str = "order.create.price_type";
    pub const ORDER_CREATE_PRICE_MARKET: &'static str = "order.create.price.market";
    pub const ORDER_CREATE_PRICE_FIXED: &'static str = "order.create.price.fixed";
    pub const ORDER_CREATE_SATS_AMOUNT: &'static str = "order.create.sats_amount";
    pub const ORDER_CREATE_SUBMIT: &'static str = "order.create.submit";
    pub const ORDER_CREATE_CANCEL: &'static str = "order.create.cancel";
    pub const ORDER_CONFIRM_HOME: &'static str = "order.confirm.home";
    // Take screen (v2 asks the amount in a dialog, for range orders only).
    pub const ORDER_TAKE_AMOUNT: &'static str = "order.take.amount";
    pub const ORDER_TAKE_AMOUNT_CONFIRM: &'static str = "order.take.amount.confirm";
    pub const ORDER_TAKE_CONFIRM: &'static str = "order.take.confirm";
    // Trade detail.
    pub const ORDER_ID: &'static str = "order.id";
    pub const ORDER_STATUS: &'static str = "order.status";
    pub const TRADE_PAY_INVOICE: &'static str = "trade.payInvoice";
    pub const TRADE_ADD_INVOICE: &'static str = "trade.addInvoice";
    pub const TRADE_FIAT_SENT: &'static str = "trade.fiatSent";
    pub const TRADE_RELEASE: &'static str = "trade.release";
    pub const TRADE_RELEASE_CONFIRM: &'static str = "trade.release.confirm";
    pub const TRADE_CANCEL: &'static str = "trade.cancel";
    pub const TRADE_CANCEL_CONFIRM: &'static str = "trade.cancel.confirm";
    pub const TRADE_DISPUTE: &'static str = "trade.dispute";
    pub const TRADE_DISPUTE_CONFIRM: &'static str = "trade.dispute.confirm";
    // Rating lives on the completed trade's detail: its stars, submit and
    // close. No control opens a separate rating screen first.
    pub const TRADE_RATE_SUBMIT: &'static str = "trade.rate.submit";
    pub const TRADE_RATE_CLOSE: &'static str = "trade.rate.close";
    // Buyer invoice and hold invoice payment.
    //
    // There is no generate/confirm pair: with a wallet connected v2 asks NWC
    // for the invoice and submits it in one step, so `invoice.nwc.text` is a
    // readout of what was submitted, not a step to drive. `invoice.manual`
    // and the manual form below are the fallback the app shows when NWC
    // cannot produce one.
    pub const INVOICE_NWC_TEXT: &'static str = "invoice.nwc.text";
    pub const INVOICE_MANUAL: &'static str = "invoice.manual";
    pub const INVOICE_AMOUNT: &'static str = "invoice.amount";
    pub const INVOICE_ORDER_ID: &'static str = "invoice.order_id";
    pub const INVOICE_TEXT: &'static str = "invoice.text";
    pub const INVOICE_SUBMIT: &'static str = "invoice.submit";
    pub const INVOICE_CANCEL: &'static str = "invoice.cancel";
    /// Readout: why the daemon refused the last submitted buyer invoice.
    pub const INVOICE_ERROR: &'static str = "invoice.error";
    /// Readout: the app's own verdict on the typed invoice, as a stable
    /// word; absent while it has not judged it.
    pub const INVOICE_CHECK: &'static str = "invoice.check";
    pub const PAY_INVOICE_TEXT: &'static str = "pay.invoice.text";
    pub const PAY_ORDER_ID: &'static str = "pay.order_id";
    pub const PAY_NWC: &'static str = "pay.nwc";
    pub const PAY_CANCEL: &'static str = "pay.cancel";

    /// Every static identifier, for contract tests.
    pub const ALL: &'static [&'static str] = &[
        Self::ENV_MARKER,
        Self::APPBAR_DRAWER,
        Self::APPBAR_BACK,
        Self::NAV_ORDER_BOOK,
        Self::NAV_TRADES,
        Self::TRADES_EMPTY,
        Self::TRADES_ERROR,
        Self::NAV_CHAT,
        Self::DRAWER_ACCOUNT,
        Self::DRAWER_SETTINGS,
        Self::DRAWER_ABOUT,
        Self::WALKTHROUGH_SKIP,
        Self::WALKTHROUGH_NEXT,
        Self::WALKTHROUGH_DONE,
        Self::COMMUNITY_DONE,
        Self::KEYS_GENERATE,
        Self::KEYS_GENERATE_CONFIRM,
        Self::KEYS_IMPORT,
        Self::KEYS_PUBLIC_KEY,
        Self::SETTINGS_MOSTRO_NODE,
        Self::SETTINGS_MOSTRO_NODE_PUBKEY,
        Self::NODE_CUSTOM_PUBKEY,
        Self::NODE_CUSTOM_CONFIRM,
        Self::NODE_CUSTOM_CANCEL,
        Self::SETTINGS_RELAYS,
        Self::SETTINGS_RELAYS_ADD,
        Self::SETTINGS_RELAYS_ADD_URL,
        Self::SETTINGS_RELAYS_ADD_CONFIRM,
        Self::SETTINGS_RELAYS_ADD_CANCEL,
        Self::SETTINGS_WALLET,
        Self::WALLET_SETTINGS_DISCONNECT,
        Self::WALLET_NWC_URI,
        Self::WALLET_NWC_PASTE,
        Self::WALLET_NWC_CONNECT,
        Self::WALLET_CONNECTION,
        Self::ORDER_BOOK_TAB_BUY,
        Self::ORDER_BOOK_TAB_SELL,
        Self::ORDER_ADD_FAB,
        Self::ORDER_ADD_BUY,
        Self::ORDER_ADD_SELL,
        Self::ORDER_CREATE_CURRENCY,
        Self::ORDER_CREATE_CURRENCY_SEARCH,
        Self::ORDER_CREATE_FIAT_AMOUNT,
        Self::ORDER_CREATE_RANGE,
        Self::ORDER_CREATE_AMOUNT_RANGE,
        Self::ORDER_CREATE_FIAT_MIN,
        Self::ORDER_CREATE_FIAT_MAX,
        Self::ORDER_CREATE_PAYMENT_METHOD_ADD,
        Self::ORDER_CREATE_PAYMENT_METHOD_CUSTOM_OPEN,
        Self::ORDER_CREATE_PAYMENT_METHOD,
        Self::ORDER_CREATE_PAYMENT_METHOD_CUSTOM_ADD,
        Self::ORDER_CREATE_PAYMENT_METHODS_CONFIRM,
        Self::ORDER_CREATE_PRICE_TYPE,
        Self::ORDER_CREATE_PRICE_MARKET,
        Self::ORDER_CREATE_PRICE_FIXED,
        Self::ORDER_CREATE_SATS_AMOUNT,
        Self::ORDER_CREATE_SUBMIT,
        Self::ORDER_CREATE_CANCEL,
        Self::ORDER_CONFIRM_HOME,
        Self::ORDER_TAKE_AMOUNT,
        Self::ORDER_TAKE_AMOUNT_CONFIRM,
        Self::ORDER_TAKE_CONFIRM,
        Self::ORDER_ID,
        Self::ORDER_STATUS,
        Self::TRADE_PAY_INVOICE,
        Self::TRADE_ADD_INVOICE,
        Self::TRADE_FIAT_SENT,
        Self::TRADE_RELEASE,
        Self::TRADE_RELEASE_CONFIRM,
        Self::TRADE_CANCEL,
        Self::TRADE_CANCEL_CONFIRM,
        Self::TRADE_DISPUTE,
        Self::TRADE_DISPUTE_CONFIRM,
        Self::TRADE_RATE_SUBMIT,
        Self::TRADE_RATE_CLOSE,
        Self::INVOICE_NWC_TEXT,
        Self::INVOICE_MANUAL,
        Self::INVOICE_AMOUNT,
        Self::INVOICE_ORDER_ID,
        Self::INVOICE_TEXT,
        Self::INVOICE_SUBMIT,
        Self::INVOICE_CANCEL,
        Self::INVOICE_ERROR,
        Self::INVOICE_CHECK,
        Self::PAY_INVOICE_TEXT,
        Self::PAY_ORDER_ID,
        Self::PAY_NWC,
        Self::PAY_CANCEL,
    ];

    /// Card of the node `pubkey` (hex) on the first run's node choice.
    pub fn community_card(pubkey: &str) -> String {
        format!("onboarding.community.card.{pubkey}")
    }

    /// Row of `order_id` in the order book.
    pub fn order_book_item(order_id: &str) -> String {
        format!("order.book.item.{order_id}")
    }

    /// Row of `order_id` in the trades list.
    pub fn trades_item(order_id: &str) -> String {
        format!("trades.item.{order_id}")
    }

    /// Star `score` (1-5) on the rating screen.
    pub fn rate_star(score: u8) -> String {
        format!("trade.rate.star.{score}")
    }

    /// Currency option in the create-order currency picker.
    pub fn currency_option(code: &str) -> String {
        format!("order.create.currency.{code}")
    }

    /// Row of a configured relay in the relays card.
    ///
    /// The url is normalized the way the app normalizes it, so a relay named
    /// with and without a trailing slash is one row and not two.
    pub fn relay_item(url: &str) -> String {
        format!("settings.relays.item.{}", normalize_relay_url(url))
    }

    /// Delete control of a configured relay.
    pub fn relay_delete(url: &str) -> String {
        format!("{}.delete", Self::relay_item(url))
    }
}

/// Mirrors `AutomationIds._normalizeRelayUrl` in the v2 app.
fn normalize_relay_url(url: &str) -> &str {
    url.trim().trim_end_matches('/')
}

/// Path of the v2 app's identifier registry, relative to its checkout.
pub const APP_IDS_FILE: &str = "lib/core/automation/automation_ids.dart";

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    /// Guard: `ALL` drives the contract and uniqueness tests, so a constant
    /// missing from it is silently uncovered. Every declared identifier must
    /// be listed.
    #[test]
    fn every_declared_identifier_is_listed_in_all() {
        let src = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/src/selectors.rs"))
            .unwrap();
        let declared: Vec<String> = src
            .lines()
            .filter_map(|l| l.trim().strip_prefix("pub const "))
            .filter_map(|l| l.split_once("&'static str = \""))
            .filter_map(|(_, rest)| rest.split_once('"'))
            .map(|(value, _)| value.to_owned())
            .collect();
        assert!(declared.len() > 10, "the parser found nothing to check");
        for id in &declared {
            assert!(
                Ids::ALL.contains(&id.as_str()),
                "{id} is declared but missing from Ids::ALL"
            );
        }
    }

    /// Names the app v2 checkout whose registry the contract test reads.
    const APP_CHECKOUT_ENV: &str = "MORTSOM_APP_CHECKOUT";

    /// Prefixes of the keyed identifiers the adapter builds at run time.
    const DYNAMIC_PREFIXES: [&str; 6] = [
        "settings.relays.item.$",
        "order.book.item.$",
        "trades.item.$",
        "order.create.currency.$",
        "trade.rate.star.$",
        "onboarding.community.card.$",
    ];

    /// Every identifier and dynamic prefix the adapter uses that the
    /// registry source does not carry literally.
    fn contract_gaps(dart: &str) -> Vec<String> {
        let ids = Ids::ALL
            .iter()
            .filter(|id| !dart.contains(&format!("'{id}'")));
        let prefixes = DYNAMIC_PREFIXES.iter().filter(|p| !dart.contains(**p));
        ids.chain(prefixes).map(|gap| (*gap).to_owned()).collect()
    }

    /// The registry inside `checkout`; a relative checkout is read from the
    /// workspace root, where `cargo test` is run, not from this crate.
    fn registry_path(checkout: &std::path::Path) -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../..")
            .join(checkout)
            .join(APP_IDS_FILE)
    }

    /// Contract test: every identifier the adapter uses exists literally in
    /// the registry of the app checkout `MORTSOM_APP_CHECKOUT` names. Unset,
    /// it says so and skips; set, a registry it cannot read fails it, so a
    /// check that was asked for never passes without running.
    #[test]
    fn selectors_match_the_v2_app_contract() {
        let Some(checkout) = std::env::var_os(APP_CHECKOUT_ENV) else {
            eprintln!("skipping: set {APP_CHECKOUT_ENV} to an app v2 checkout to check it");
            return;
        };
        let path = registry_path(checkout.as_ref());
        let dart = std::fs::read_to_string(&path).unwrap_or_else(|error| {
            panic!(
                "{APP_CHECKOUT_ENV}: cannot read {}: {error}",
                path.display()
            )
        });
        let gaps = contract_gaps(&dart);
        assert!(
            gaps.is_empty(),
            "missing from {}: {}",
            path.display(),
            gaps.join(", ")
        );
    }

    /// A registry carrying every identifier and prefix the adapter uses.
    fn full_registry() -> String {
        let ids = Ids::ALL.iter().map(|id| format!("'{id}'"));
        ids.chain(DYNAMIC_PREFIXES.iter().map(|p| format!("'{p}")))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn a_registry_missing_an_identifier_is_a_contract_gap() {
        let full = full_registry();
        assert!(
            contract_gaps(&full).is_empty(),
            "{:?}",
            contract_gaps(&full)
        );
        let dropped = Ids::ALL[0];
        let without = full.replace(&format!("'{dropped}'"), "");
        assert_eq!(contract_gaps(&without), vec![dropped.to_owned()]);
    }

    #[test]
    fn a_registry_missing_a_dynamic_prefix_is_a_contract_gap() {
        let without = full_registry().replace("trades.item.$", "");
        assert_eq!(contract_gaps(&without), vec!["trades.item.$".to_owned()]);
    }

    #[test]
    fn a_relative_checkout_is_read_from_the_workspace_root() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../..");
        assert_eq!(
            registry_path("../appv2".as_ref()),
            root.join("../appv2").join(APP_IDS_FILE)
        );
        assert_eq!(
            registry_path("/srv/app".as_ref()),
            std::path::Path::new("/srv/app").join(APP_IDS_FILE)
        );
    }

    #[test]
    fn identifiers_are_unique_and_dotted() {
        let mut seen = std::collections::BTreeSet::new();
        for id in Ids::ALL {
            assert!(seen.insert(*id), "duplicate id {id}");
            assert!(id.contains('.') && !id.contains(' '), "{id}");
        }
    }

    #[test]
    fn dynamic_ids_embed_their_key() {
        assert_eq!(Ids::order_book_item("o1"), "order.book.item.o1");
        assert_eq!(Ids::trades_item("o1"), "trades.item.o1");
        assert_eq!(Ids::currency_option("USD"), "order.create.currency.USD");
        assert_eq!(Ids::rate_star(5), "trade.rate.star.5");
        assert_eq!(Ids::community_card("ab"), "onboarding.community.card.ab");
        assert_eq!(
            Ids::relay_item("ws://127.0.0.1:7000"),
            "settings.relays.item.ws://127.0.0.1:7000"
        );
        assert_eq!(
            Ids::relay_delete("ws://127.0.0.1:7000"),
            "settings.relays.item.ws://127.0.0.1:7000.delete"
        );
    }

    /// The app normalizes a relay url before keying its row; a harness that
    /// did not would look for a row that is never rendered.
    #[test]
    fn a_relay_keeps_one_identifier_however_its_url_is_written() {
        let canonical = "settings.relays.item.ws://127.0.0.1:7000";
        for written in [
            "ws://127.0.0.1:7000",
            "ws://127.0.0.1:7000/",
            "ws://127.0.0.1:7000//",
            "  ws://127.0.0.1:7000/  ",
        ] {
            assert_eq!(Ids::relay_item(written), canonical, "{written}");
        }
    }
}
