//! Explicit continuation on a different, verified regtest stack. This never
//! migrates, closes, edits or deletes an ambiguous legacy recovery journal.
use std::{
    collections::BTreeSet,
    path::{Path, PathBuf},
};

use mortsom_common::{Actor, Lane, RunId};
use serde::Serialize;
use thiserror::Error;

use crate::{
    Config,
    recovery_abandon::StackInspector,
    recovery_store::FileJournal,
    run::WalletFactory,
    stack::{ProjectOwner, RecordedIdentity, StackFingerprint, StackSnapshot},
};

/// Refusal to start isolated work is an environment error, never a product result.
#[derive(Debug, Error)]
pub enum IsolationError {
    /// The request cannot safely preserve the selected legacy journals.
    #[error("legacy recovery isolation refused: {0}")]
    Refused(String),
    /// Recorded state or live facts cannot be inspected.
    #[error("legacy recovery isolation inspection failed: {0}")]
    Inspection(String),
}

/// Public facts about a journal left unchanged by this invocation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
struct PreservedRun {
    run_id: RunId,
    original_stack: StackFingerprint,
}

/// Proof bound to one current environment; cannot be constructed from a flag
/// or deserialized from yesterday's evidence. Revalidated before journal creation.
#[derive(Debug, Clone, Serialize)]
pub struct ValidatedIsolation {
    preserved_pending: Vec<PreservedRun>,
    current_stack: StackFingerprint,
    current_stack_dir: PathBuf,
    current_node_networks: std::collections::BTreeMap<String, String>,
    statement: String,
}

/// Inputs for inspecting the current environment, never historical backfill.
pub struct IsolationContext<'a> {
    /// Operator's private journal inventory.
    pub base: &'a Path,
    /// Exact pending run IDs acknowledged for this invocation.
    pub confirmed: &'a [RunId],
    /// Current configuration.
    pub config: &'a Config,
    /// Current stack snapshot.
    pub stack: &'a StackSnapshot,
    /// Selected owned stack directory.
    pub stack_dir: &'a Path,
}

/// Checks stopped legacy storage, exact acknowledgements, current ownership,
/// wallet binding and live regtest networks before any app build or startup.
pub async fn authorize(
    context: &IsolationContext<'_>,
    inspector: &dyn StackInspector,
    wallets: &WalletFactory<'_>,
) -> Result<ValidatedIsolation, IsolationError> {
    current_policy(context.config, context.stack)?;
    let preserved_pending = inventory(context.base, context.confirmed, context.stack)?;
    let directory = owned_current_stack(context, inspector).await?;
    let networks = current_networks(context.config, wallets).await?;
    Ok(ValidatedIsolation {
        preserved_pending,
        current_stack: context.stack.fingerprint().map_err(inspect)?,
        current_stack_dir: directory,
        current_node_networks: networks,
        statement: "Legacy recovery remains pending and unchanged. Its environment, order and payment resolution are NOT verified. Only this new regtest run is authorized.".into(),
    })
}

async fn owned_current_stack(
    context: &IsolationContext<'_>,
    inspector: &dyn StackInspector,
) -> Result<PathBuf, IsolationError> {
    let directory = context.stack_dir.canonicalize().map_err(inspect)?;
    let facts = inspector
        .facts(&directory, &directory)
        .await
        .map_err(inspect)?;
    let matches = |identity: &RecordedIdentity| match identity {
        RecordedIdentity::Present(recorded) => {
            context.stack.matches_recorded(recorded).unwrap_or(false)
        }
        _ => false,
    };
    if facts.project != ProjectOwner::Directory(directory.clone())
        || facts.selected.0 != directory
        || !matches(&facts.selected.1)
        || !facts.project_identity.as_ref().is_some_and(matches)
    {
        return Err(refuse(
            "the current Compose project and descriptor must belong to the selected stack",
        ));
    }
    Ok(directory)
}

async fn current_networks(
    config: &Config,
    wallets: &WalletFactory<'_>,
) -> Result<std::collections::BTreeMap<String, String>, IsolationError> {
    let mut networks = std::collections::BTreeMap::new();
    for (label, wallet) in [
        ("alice", &config.wallets.alice),
        ("bob", &config.wallets.bob),
    ] {
        let handles = wallets(label, wallet).await.map_err(inspect)?;
        if handles.node.is_none() || handles.network.as_deref() != Some("regtest") {
            return Err(refuse(format!(
                "current {label} wallet must report regtest"
            )));
        }
        networks.insert(label.to_owned(), "regtest".to_owned());
    }
    Ok(networks)
}

impl ValidatedIsolation {
    /// Repeat the inventory/storage/configuration gates at the creation boundary.
    pub(crate) fn revalidate(
        &self,
        base: &Path,
        config: &Config,
        stack: &StackSnapshot,
        stack_dir: Option<&Path>,
    ) -> Result<(), IsolationError> {
        current_policy(config, stack)?;
        if stack.fingerprint().map_err(inspect)? != self.current_stack
            || stack_dir != Some(self.current_stack_dir.as_path())
        {
            return Err(refuse(
                "the current stack changed after isolation was authorized",
            ));
        }
        let ids: Vec<_> = self
            .preserved_pending
            .iter()
            .map(|run| run.run_id.clone())
            .collect();
        if inventory(base, &ids, stack)? != self.preserved_pending {
            return Err(refuse("the pending recovery inventory changed"));
        }
        Ok(())
    }

    /// Always identifies the unresolved runs and never describes them as clean.
    pub fn warning(&self) -> String {
        format!(
            "{} Preserved pending runs: {}",
            self.statement,
            self.preserved_pending
                .iter()
                .map(|run| run.run_id.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        )
    }
}

fn current_policy(config: &Config, stack: &StackSnapshot) -> Result<(), IsolationError> {
    if config.run.allow_mainnet || config.run.lane == Lane::Canary {
        return Err(refuse(
            "mainnet-enabled and canary runs cannot preserve pending legacy recovery",
        ));
    }
    stack.validate_config(config).map_err(inspect)
}

fn inventory(
    base: &Path,
    confirmed: &[RunId],
    stack: &StackSnapshot,
) -> Result<Vec<PreservedRun>, IsolationError> {
    let pending = FileJournal::pending(base).map_err(inspect)?;
    let requested: BTreeSet<_> = confirmed.iter().map(RunId::as_str).collect();
    let actual: BTreeSet<_> = pending.iter().map(|run| run.run_id.as_str()).collect();
    if requested.is_empty() || requested.len() != confirmed.len() || requested != actual {
        return Err(refuse(format!(
            "repeat --preserve-legacy-recovery once for each exact pending legacy run ID; unknown, duplicate or omitted IDs are refused. Pending runs: {}",
            actual.into_iter().collect::<Vec<_>>().join(", ")
        )));
    }
    let mut preserved = Vec::new();
    for manifest in pending {
        if manifest.environment.is_some() || manifest.resolution_verified {
            return Err(refuse(format!(
                "run {} is not an unresolved legacy journal; use cleanup or verified abandonment",
                manifest.run_id
            )));
        }
        if stack.matches_recorded(&manifest.stack).map_err(inspect)? {
            return Err(refuse(format!(
                "run {} belongs to the selected stack; use cleanup",
                manifest.run_id
            )));
        }
        let root = base.join(manifest.run_id.as_str());
        for actor in Actor::ALL {
            // Missing or unsafe storage also refuses. No active marker is removed.
            mortsom_desktop_driver::storage::ensure_stopped(
                &root.join("actors").join(actor.as_str()),
            )
            .map_err(inspect)?;
        }
        preserved.push(PreservedRun {
            run_id: manifest.run_id,
            original_stack: manifest.stack,
        });
    }
    Ok(preserved)
}

fn refuse(message: impl Into<String>) -> IsolationError {
    IsolationError::Refused(message.into())
}
fn inspect(error: impl std::fmt::Display) -> IsolationError {
    IsolationError::Inspection(error.to_string())
}

/// Conditional guidance, never a claim that historical safety was proved.
pub fn legacy_hint(run: &RunId) -> String {
    format!(
        "Legacy run {run} lacks historical environment evidence and cannot be abandoned. To keep it pending and attempt work on a different verified Mortsom regtest stack, add `--preserve-legacy-recovery {run}` to `mortsom run`. Mainnet/canary and unsafe or active storage remain refused; never edit or delete the journal."
    )
}
