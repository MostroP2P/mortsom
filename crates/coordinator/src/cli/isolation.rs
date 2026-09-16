//! Invocation-scoped legacy preservation; default recovery remains fail-closed.
use std::path::Path;

use mortsom_common::RunId;
use mortsom_ui_driver::process::TokioRunner;

use crate::{
    Config,
    doctor::{HostProbe, RealHost},
    recovery_abandon::LiveStacks,
    recovery_isolation::{IsolationContext, ValidatedIsolation, authorize},
    recovery_store::FileJournal,
    stack::StackSnapshot,
};

pub(super) async fn admit(
    confirmed: &[RunId],
    config: &Config,
    stack: &StackSnapshot,
    stack_dir: &Path,
) -> Result<Option<ValidatedIsolation>, String> {
    let home = RealHost
        .env("HOME")
        .ok_or("HOME is required for desktop recovery")?;
    let base = FileJournal::directory(Path::new(&home))?;
    if confirmed.is_empty() {
        return FileJournal::refuse_pending(&base)
            .map(|()| None)
            .map_err(|pending| pending.message(super::use_color_on_stderr()));
    }
    let context = IsolationContext {
        base: &base,
        confirmed,
        config,
        stack,
        stack_dir,
    };
    let isolation = authorize(
        &context,
        &LiveStacks {
            runner: &TokioRunner,
        },
        &super::run::wallet_factory,
    )
    .await
    .map_err(|error| error.to_string())?;
    eprintln!("{}", isolation.warning());
    Ok(Some(isolation))
}
