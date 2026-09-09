//! Readers for the JSON `lncli` prints and the service table Compose prints.
use serde::Deserialize;

use super::StackError;

/// One Compose service and the state Compose reports for it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Service {
    /// Compose service name.
    pub name: String,
    /// `running`, `exited`, `restarting` and so on.
    pub state: String,
}

impl Service {
    /// Whether Compose reports the service as running.
    pub fn is_running(&self) -> bool {
        self.state == "running"
    }
}

/// Lines of `docker compose ps --format '{{.Service}} {{.State}}'`.
pub fn parse_services(raw: &str) -> Vec<Service> {
    raw.lines()
        .filter_map(|line| {
            let mut words = line.split_whitespace();
            let name = words.next()?;
            let state = words.next()?;
            Some(Service {
                name: name.to_owned(),
                state: state.to_owned(),
            })
        })
        .collect()
}

#[derive(Deserialize)]
struct GetInfo {
    identity_pubkey: String,
}

#[derive(Deserialize)]
struct NewAddress {
    address: String,
}

#[derive(Deserialize)]
struct WalletBalance {
    confirmed_balance: String,
}

#[derive(Deserialize)]
struct Channel {
    #[serde(default)]
    active: bool,
}

#[derive(Deserialize)]
struct ListChannels {
    #[serde(default)]
    channels: Vec<Channel>,
}

fn parse<T: for<'de> Deserialize<'de>>(what: &str, raw: &str) -> Result<T, StackError> {
    serde_json::from_str(raw.trim())
        .map_err(|error| StackError::Configuration(format!("{what}: unreadable reply: {error}")))
}

pub fn identity_pubkey(getinfo: &str) -> Result<String, StackError> {
    Ok(parse::<GetInfo>("getinfo", getinfo)?.identity_pubkey)
}

pub fn new_address(newaddress: &str) -> Result<String, StackError> {
    Ok(parse::<NewAddress>("newaddress", newaddress)?.address)
}

/// LND reports balances as decimal strings.
pub fn confirmed_balance(walletbalance: &str) -> Result<u64, StackError> {
    let raw = parse::<WalletBalance>("walletbalance", walletbalance)?.confirmed_balance;
    raw.parse().map_err(|error| {
        StackError::Configuration(format!("walletbalance: {raw:?} is not a number: {error}"))
    })
}

pub fn channel_count(listchannels: &str) -> Result<usize, StackError> {
    Ok(parse::<ListChannels>("listchannels", listchannels)?
        .channels
        .len())
}

pub fn active_channel_count(listchannels: &str) -> Result<usize, StackError> {
    Ok(parse::<ListChannels>("listchannels", listchannels)?
        .channels
        .iter()
        .filter(|channel| channel.active)
        .count())
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn reads_the_replies_lncli_prints() {
        assert_eq!(
            identity_pubkey(r#"{"version":"0.19.1","identity_pubkey":"02aa"}"#).unwrap(),
            "02aa"
        );
        assert_eq!(
            new_address(r#"{"address":"bcrt1qxy"}"#).unwrap(),
            "bcrt1qxy"
        );
        assert_eq!(
            confirmed_balance(r#"{"total_balance":"1","confirmed_balance":"99000"}"#).unwrap(),
            99_000
        );
        let channels = r#"{"channels":[{"active":true},{"active":false},{}]}"#;
        assert_eq!(channel_count(channels).unwrap(), 3);
        assert_eq!(active_channel_count(channels).unwrap(), 1);
        assert_eq!(channel_count(r#"{}"#).unwrap(), 0);
    }

    #[test]
    fn refuses_replies_that_are_not_the_expected_json() {
        assert!(identity_pubkey("[lncli] rpc error: server is starting").is_err());
        assert!(identity_pubkey(r#"{"alias":"carol"}"#).is_err());
        assert!(confirmed_balance(r#"{"confirmed_balance":"lots"}"#).is_err());
    }

    #[test]
    fn reads_the_service_table_compose_prints() {
        let services = parse_services("bitcoind running\nmostro restarting\n\n");
        assert_eq!(services.len(), 2);
        assert!(services[0].is_running());
        assert!(!services[1].is_running());
        assert!(parse_services("").is_empty());
    }
}
