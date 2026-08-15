//! Mortsom scenario kit (spec §7, §8, §12.1): typed domain actions over the
//! `AppAdapter` contract, bounded waits, correlation assertions, cap
//! accounting, cleanup registration and the ordered timeline. It never
//! imports a selector, a Flutter route, a Rust-core type or a localized
//! string; adapters own those.

pub mod actions;
pub mod budget;
pub mod cleanup;
pub mod context;
pub mod evidence;
pub mod outcome;
pub mod registry;
pub mod timeline;
pub mod waits;

pub use budget::{Budget, BudgetError, SpendCaps};
pub use cleanup::{CleanupEntry, CleanupRegistry, CleanupReport};
pub use context::{ActorSession, Context, MainnetClearance, ScenarioError};
pub use evidence::EvidenceSink;
pub use outcome::{Outcome, ScenarioResult};
pub use registry::{ScenarioDef, ScenarioFn, all, find, select};
pub use timeline::{Timeline, TimelineEntry, TimelineKind};
