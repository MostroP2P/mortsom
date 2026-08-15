//! Observer scope: which events belong to this run. Pure and unit tested.

use mostro_core::prelude::{
    NOSTR_DISPUTE_EVENT_KIND, NOSTR_INFO_EVENT_KIND, NOSTR_ORDER_EVENT_KIND,
};
use nostr_sdk::prelude::{Event, Filter, Kind, PublicKey};

/// Kinds published by the daemon that describe it rather than an order.
const DAEMON_KINDS: [Kind; 3] = [
    Kind::Custom(NOSTR_INFO_EVENT_KIND),
    Kind::RelayList,
    Kind::Metadata,
];

/// What the observer subscribes to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ObserverScope {
    /// The daemon under test.
    pub mostro_pubkey: PublicKey,
    /// Alice and Bob identity/trade keys known to the run.
    pub actor_pubkeys: Vec<PublicKey>,
    /// Order ids created by this run (`d` tag of kind 38383/38386 events).
    pub order_ids: Vec<String>,
}

impl ObserverScope {
    /// A scope with no actors or orders yet: only daemon self-description.
    pub fn daemon_only(mostro_pubkey: PublicKey) -> Self {
        Self {
            mostro_pubkey,
            actor_pubkeys: Vec::new(),
            order_ids: Vec::new(),
        }
    }

    /// Registers a run-created order id (idempotent).
    pub fn add_order(&mut self, order_id: &str) {
        if !self.order_ids.iter().any(|id| id == order_id) {
            self.order_ids.push(order_id.to_owned());
        }
    }

    /// Registers an actor public key (idempotent).
    pub fn add_actor(&mut self, pubkey: PublicKey) {
        if !self.actor_pubkeys.contains(&pubkey) {
            self.actor_pubkeys.push(pubkey);
        }
    }

    /// Relay filters for the current scope. `limit(0)` asks the relay for
    /// live delivery only; nostr-rs-relay honours it and the observer never
    /// needs history because it subscribes before any run action.
    pub fn filters(&self) -> Vec<Filter> {
        let mut filters = vec![
            Filter::new()
                .author(self.mostro_pubkey)
                .kinds(DAEMON_KINDS)
                .limit(0),
            // Protocol traffic to and from the daemon (metadata only, the
            // content is encrypted): on an isolated run relay every such
            // message belongs to this run, which is what makes "no duplicate
            // action after a restart" observable (spec §8).
            Filter::new()
                .kind(Kind::PrivateDirectMessage)
                .pubkey(self.mostro_pubkey)
                .limit(0),
            Filter::new()
                .kind(Kind::PrivateDirectMessage)
                .author(self.mostro_pubkey)
                .limit(0),
        ];
        if !self.order_ids.is_empty() {
            filters.push(
                Filter::new()
                    .author(self.mostro_pubkey)
                    .kinds([
                        Kind::Custom(NOSTR_ORDER_EVENT_KIND),
                        Kind::Custom(NOSTR_DISPUTE_EVENT_KIND),
                    ])
                    .identifiers(self.order_ids.clone())
                    .limit(0),
            );
        }
        if !self.actor_pubkeys.is_empty() {
            filters.push(
                Filter::new()
                    .kind(Kind::PrivateDirectMessage)
                    .pubkeys(self.actor_pubkeys.clone())
                    .limit(0),
            );
            filters.push(
                Filter::new()
                    .kind(Kind::PrivateDirectMessage)
                    .authors(self.actor_pubkeys.clone())
                    .limit(0),
            );
        }
        filters
    }

    /// Client-side check mirroring [`ObserverScope::filters`], because a
    /// relay may over-deliver and the log must contain only run evidence.
    pub fn matches(&self, event: &Event) -> bool {
        if event.pubkey == self.mostro_pubkey {
            if DAEMON_KINDS.contains(&event.kind) {
                return true;
            }
            if event.kind == Kind::Custom(NOSTR_ORDER_EVENT_KIND)
                || event.kind == Kind::Custom(NOSTR_DISPUTE_EVENT_KIND)
            {
                return event
                    .tags
                    .identifier()
                    .is_some_and(|d| self.order_ids.contains(&d));
            }
        }
        if event.kind == Kind::PrivateDirectMessage {
            // Spec §4.3: the observer records events for run identities, so
            // any DM addressed to or authored by the daemon or an actor is
            // evidence, even one exchanged directly between Alice and Bob.
            let to_run_identity = event
                .tags
                .public_keys()
                .any(|p| p == self.mostro_pubkey || self.actor_pubkeys.contains(&p));
            return to_run_identity
                || event.pubkey == self.mostro_pubkey
                || self.actor_pubkeys.contains(&event.pubkey);
        }
        false
    }
}

/// Whether the event content is stored: encrypted direct messages are not.
pub fn keeps_content(kind: Kind) -> bool {
    kind != Kind::PrivateDirectMessage
}
