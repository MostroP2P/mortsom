//! Mortsom scenario kit (spec §7, §8, §12.1): typed domain actions over the
//! `AppAdapter` contract, bounded waits, correlation assertions, cap
//! accounting, cleanup registration and the ordered timeline. It never
//! imports a selector, a Flutter route, a Rust-core type or a localized
//! string; adapters own those.

pub mod actions;
pub mod budget;
pub mod cancel;
pub mod cleanup;
pub mod context;
pub mod evidence;
pub mod outcome;
pub mod recovery;
pub mod registry;
pub mod timeline;
pub mod waits;

#[cfg(test)]
pub(crate) mod test_support;

pub use budget::{Budget, BudgetError, SpendCaps};
pub use cleanup::{CleanupEntry, CleanupRegistry, CleanupReport};
pub use context::{ActorSession, Context, MainnetClearance, RecoveryTiming, ScenarioError};
pub use evidence::EvidenceSink;
pub use outcome::{Outcome, ScenarioResult};
pub use recovery::{NoRelayControl, RelayControl};
pub use registry::{ScenarioDef, ScenarioFn, all, find, select};
pub use timeline::{Timeline, TimelineEntry, TimelineKind};
