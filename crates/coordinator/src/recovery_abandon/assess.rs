//! Pure decisions over recorded manifest facts and inspected stack facts.
use std::path::{Path, PathBuf};

use async_trait::async_trait;

use crate::recovery_store::{AbandonReason, RecoveryManifest};
use crate::stack::{ProjectOwner, RecordedIdentity, StackFingerprint};

/// What the host shows about stacks right now.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LiveFacts {
    /// Identity in the directory the journal recorded, with that directory.
    pub recorded: (PathBuf, RecordedIdentity),
    /// Identity in the directory the operator selected.
    pub selected: (PathBuf, RecordedIdentity),
    /// Who holds the Compose project.
    pub project: ProjectOwner,
    /// Identity in the project owner's directory, when there is one.
    pub project_identity: Option<RecordedIdentity>,
}

/// Read-only access to the stack facts, so tests inject fakes.
#[async_trait]
pub trait StackInspector: Send + Sync {
    /// Inspects `recorded` and `selected` and the Compose project.
    async fn facts(&self, recorded: &Path, selected: &Path) -> Result<LiveFacts, String>;
    /// Compare identities using the journal's digest schema, not today's schema.
    async fn facts_for(
        &self,
        recorded: &Path,
        selected: &Path,
        _original: &StackFingerprint,
    ) -> Result<LiveFacts, String> {
        self.facts(recorded, selected).await
    }
}

/// Whether a recorded stack identity still exists anywhere on the host.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StackPresence {
    /// The identity is still recorded; holds where.
    Live(String),
    /// Nothing of it remains; a different stack may be present.
    Destroyed {
        /// Whether another completed stack is present.
        current_stack_present: bool,
    },
    /// The facts cannot prove either; holds why.
    Unprovable(String),
}

/// Decides where `original` stands. Anything unreadable or unowned fails
/// closed as [`StackPresence::Unprovable`].
pub fn presence(original: &StackFingerprint, facts: &LiveFacts) -> StackPresence {
    let mut identities = vec![
        (facts.recorded.0.display().to_string(), &facts.recorded.1),
        (facts.selected.0.display().to_string(), &facts.selected.1),
    ];
    if let (ProjectOwner::Directory(dir), Some(identity)) =
        (&facts.project, facts.project_identity.as_ref())
    {
        identities.push((dir.display().to_string(), identity));
    }
    if let Some((place, _)) = identities
        .iter()
        .find(|(_, identity)| matches!(identity, RecordedIdentity::Present(fp) if fp == original))
    {
        return StackPresence::Live(format!("its identity is recorded in {place}"));
    }
    if let Some((place, why)) = identities
        .iter()
        .find_map(|(place, identity)| match identity {
            RecordedIdentity::Unreadable(why) => Some((place, why)),
            _ => None,
        })
    {
        return StackPresence::Unprovable(format!("{place}: {why}"));
    }
    match (&facts.project, &facts.project_identity) {
        (ProjectOwner::Unprovable(why), _) => {
            return StackPresence::Unprovable(format!("the Compose project: {why}"));
        }
        (ProjectOwner::Directory(dir), None | Some(RecordedIdentity::Absent)) => {
            return StackPresence::Unprovable(format!(
                "containers started from {} carry no recorded identity",
                dir.display()
            ));
        }
        _ => {}
    }
    StackPresence::Destroyed {
        current_stack_present: identities
            .iter()
            .any(|(_, identity)| matches!(identity, RecordedIdentity::Present(_))),
    }
}

/// The recorded-fact gates: an open journal, an allowed reason and a
/// Mortsom-owned regtest environment on the stack's own nodes. Returns the
/// names of the checks that passed.
pub fn gate(manifest: &RecoveryManifest, reason: AbandonReason) -> Result<Vec<String>, String> {
    let run = &manifest.run_id;
    if !manifest.state().is_pending() {
        return Err(format!(
            "recovery {run} is {}, not pending",
            manifest.state().as_str()
        ));
    }
    if reason != AbandonReason::StackDestroyed {
        return Err(format!("reason `{}` is not allowed", reason.as_str()));
    }
    let Some(environment) = manifest.environment.as_ref() else {
        return Err(format!(
            "recovery {run} predates recorded ownership, network and wallet facts; it cannot be \
             proved to be a Mortsom-owned regtest run and cannot be abandoned"
        ));
    };
    if let Some(refusal) = environment.abandon_refusal() {
        return Err(format!("recovery {run} cannot be abandoned: {refusal}"));
    }
    Ok(vec![
        "journal is pending".into(),
        format!("reason `{}` is allowed", reason.as_str()),
        format!(
            "lane `{}` without mainnet spending",
            environment.lane.as_str()
        ),
        "stack created and owned by Mortsom".into(),
        "alice and bob used the stack's own LND nodes".into(),
        "both nodes reported regtest".into(),
    ])
}
