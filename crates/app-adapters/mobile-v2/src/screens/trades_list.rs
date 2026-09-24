//! My Trades, read only once the app says the list loaded.
//!
//! The list has four states: loading, failed, loaded empty and loaded with
//! rows. Only the last three carry an identifier. A list still loading has
//! no rows either, so an empty read proves nothing until `trades.empty`
//! says the list loaded with nothing to show.

use mortsom_app_contract::{AdapterError, DeviceContext};

use super::nav::go_home;
use super::orders::{TRADES_ITEM_PREFIX, ids_with_prefix, page_source};
use super::{SCREEN_POLL, tap};
use crate::selectors::Ids;

/// What one read of My Trades showed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TradesList {
    /// Neither rows, `trades.empty` nor `trades.error`: not loaded yet.
    Loading,
    /// `trades.error`: the app could not load the list.
    Failed,
    /// Loaded; the order ids listed, empty for `trades.empty`.
    Loaded(Vec<String>),
}

/// Classifies one accessibility dump of My Trades.
pub fn classify(xml: &str) -> TradesList {
    let rows = ids_with_prefix(xml, TRADES_ITEM_PREFIX);
    if !rows.is_empty() {
        TradesList::Loaded(rows)
    } else if has_id(xml, Ids::TRADES_ERROR) {
        TradesList::Failed
    } else if has_id(xml, Ids::TRADES_EMPTY) {
        TradesList::Loaded(Vec::new())
    } else {
        TradesList::Loading
    }
}

fn has_id(xml: &str, id: &str) -> bool {
    xml.contains(&format!("resource-id=\"{id}\""))
}

/// Ids of the trades My Trades lists, read once the list has loaded: a
/// missing id then means the app no longer lists that trade.
pub async fn trade_ids(device: &DeviceContext<'_>) -> Result<Vec<String>, AdapterError> {
    go_home(device).await?;
    tap(device, Ids::NAV_TRADES).await?;
    let deadline = tokio::time::Instant::now() + device.ui_timeout;
    loop {
        match classify(&page_source(device).await?) {
            TradesList::Loaded(ids) => return Ok(ids),
            TradesList::Failed => {
                return Err(AdapterError::product(format!(
                    "My Trades could not load the trades (`{}`)",
                    Ids::TRADES_ERROR
                )));
            }
            TradesList::Loading if tokio::time::Instant::now() >= deadline => {
                return Err(AdapterError::contract(format!(
                    "My Trades showed no row, `{}` or `{}` within {:?}",
                    Ids::TRADES_EMPTY,
                    Ids::TRADES_ERROR,
                    device.ui_timeout
                )));
            }
            TradesList::Loading => tokio::time::sleep(SCREEN_POLL).await,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dump(ids: &[&str]) -> String {
        ids.iter()
            .map(|id| format!("<node resource-id=\"{id}\"/>"))
            .collect()
    }

    #[test]
    fn rows_are_a_loaded_list() {
        assert_eq!(
            classify(&dump(&["nav.trades", "trades.item.o1", "trades.item.o2"])),
            TradesList::Loaded(vec!["o1".into(), "o2".into()])
        );
    }

    #[test]
    fn the_empty_state_is_a_loaded_list_with_no_rows() {
        assert_eq!(
            classify(&dump(&["nav.trades", Ids::TRADES_EMPTY])),
            TradesList::Loaded(Vec::new())
        );
    }

    #[test]
    fn the_error_state_is_a_failed_load() {
        assert_eq!(
            classify(&dump(&["nav.trades", Ids::TRADES_ERROR])),
            TradesList::Failed
        );
    }

    #[test]
    fn no_rows_and_no_state_is_a_list_still_loading() {
        assert_eq!(classify(&dump(&["nav.trades"])), TradesList::Loading);
    }

    #[test]
    fn an_id_that_only_starts_like_a_state_is_not_that_state() {
        assert_eq!(
            classify(&dump(&["trades.empty.hint", "trades.errors"])),
            TradesList::Loading
        );
    }
}
