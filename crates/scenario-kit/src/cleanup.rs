//! Cleanup registry (spec §7, Phase 4.2): registered the moment an order
//! could exist, executed once, idempotent, honest about what stayed open.

use std::sync::Mutex;

use mortsom_android_driver::avd::Actor;
use mortsom_app_contract::{CancelOutcome, OrderReference};
use serde::{Deserialize, Serialize};

/// One registered order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupEntry {
    /// Actor that owns the order (the maker).
    pub actor: Actor,
    /// The order.
    pub order: OrderReference,
    /// Result once cleanup ran.
    pub result: Option<CleanupResult>,
}

/// What cleanup did for one order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "result", rename_all = "kebab-case")]
pub enum CleanupResult {
    /// The adapter reported the outcome.
    Done {
        /// Outcome.
        outcome: CancelOutcome,
    },
    /// The scenario resolved the order itself (completed or cancelled).
    Resolved,
    /// Cleanup failed; the order may still be open.
    Failed {
        /// Redacted detail.
        message: String,
    },
}

/// Summary written to `cleanup.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CleanupReport {
    /// Every registered order with its result.
    pub entries: Vec<CleanupEntry>,
    /// Orders that may still be open after cleanup.
    pub unresolved: Vec<String>,
}

impl CleanupReport {
    /// True when nothing is left open.
    pub fn is_clean(&self) -> bool {
        self.unresolved.is_empty()
    }
}

/// Registry of orders that may need cancellation.
#[derive(Debug, Default)]
pub struct CleanupRegistry {
    entries: Mutex<Vec<CleanupEntry>>,
}

impl CleanupRegistry {
    /// Registers an order (idempotent by id).
    pub fn register(&self, actor: Actor, order: &OrderReference) {
        let mut entries = lock(&self.entries);
        if !entries.iter().any(|e| e.order.id == order.id) {
            entries.push(CleanupEntry {
                actor,
                order: order.clone(),
                result: None,
            });
        }
    }

    /// Marks an order as resolved by the scenario itself.
    pub fn resolved(&self, order: &OrderReference) {
        for entry in lock(&self.entries).iter_mut() {
            if entry.order.id == order.id {
                entry.result = Some(CleanupResult::Resolved);
            }
        }
    }

    /// Entries still needing cleanup, in registration order.
    pub fn pending(&self) -> Vec<CleanupEntry> {
        lock(&self.entries)
            .iter()
            .filter(|e| e.result.is_none())
            .cloned()
            .collect()
    }

    /// Records the cleanup result of one order.
    pub fn record(&self, order_id: &str, result: CleanupResult) {
        for entry in lock(&self.entries).iter_mut() {
            if entry.order.id == order_id {
                entry.result = Some(result.clone());
            }
        }
    }

    /// The report.
    pub fn report(&self) -> CleanupReport {
        let entries = lock(&self.entries).clone();
        let unresolved = entries
            .iter()
            .filter(|e| {
                !matches!(
                    e.result,
                    Some(CleanupResult::Resolved)
                        | Some(CleanupResult::Done {
                            outcome: CancelOutcome::Cancelled | CancelOutcome::AlreadyClosed
                        })
                )
            })
            .map(|e| e.order.id.clone())
            .collect();
        CleanupReport {
            entries,
            unresolved,
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|p| p.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_is_idempotent_and_reports_unresolved_honestly() {
        let registry = CleanupRegistry::default();
        let o1 = OrderReference::new("o1");
        registry.register(Actor::Alice, &o1);
        registry.register(Actor::Alice, &o1);
        registry.register(Actor::Bob, &OrderReference::new("o2"));
        registry.register(Actor::Alice, &OrderReference::new("o3"));
        assert_eq!(registry.pending().len(), 3);

        registry.resolved(&o1);
        registry.record(
            "o2",
            CleanupResult::Done {
                outcome: CancelOutcome::Cancelled,
            },
        );
        registry.record(
            "o3",
            CleanupResult::Failed {
                message: "device gone".into(),
            },
        );

        let report = registry.report();
        assert_eq!(report.unresolved, vec!["o3"]);
        assert!(!report.is_clean());
        assert!(registry.pending().is_empty(), "every entry resolved once");
    }

    #[test]
    fn not_found_counts_as_unresolved() {
        let registry = CleanupRegistry::default();
        registry.register(Actor::Alice, &OrderReference::new("o1"));
        registry.record(
            "o1",
            CleanupResult::Done {
                outcome: CancelOutcome::NotFound,
            },
        );
        assert_eq!(registry.report().unresolved, vec!["o1"]);
    }
}
