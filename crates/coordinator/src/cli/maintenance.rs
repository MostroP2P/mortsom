//! Offline artifact inspection, publication and recorded-resource cleanup.
use crate::doctor::{HostProbe, RealHost};
use mortsom_android_driver::process::TokioRunner;
use mortsom_environment::docker::DockerCli;
use std::process::ExitCode;

pub(super) fn publish_command(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    run_id: &str,
    out: &std::path::Path,
    screenshots: &str,
) -> ExitCode {
    let run_id: mortsom_common::RunId = match run_id.parse() {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let config = match crate::config::PublishSettings::load_for_run(
        config_path,
        stack_dir,
        &run_id,
        |name| RealHost.env(name),
    ) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let screenshots = match screenshots.parse() {
        Ok(policy) => policy,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let options = crate::publish::PublishOptions { screenshots };
    match crate::publish::publish_artifacts(&config, run_id.as_str(), out, options) {
        Ok(manifest) => {
            print!("{}", manifest.render());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

pub(super) fn parity_command(
    config_path: &std::path::Path,
    run_a: &str,
    run_b: &str,
    intentional: Option<&std::path::Path>,
) -> ExitCode {
    let config = match crate::config::ArtifactSettings::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let ids: Result<(mortsom_common::RunId, mortsom_common::RunId), String> =
        run_a.parse().and_then(|a| run_b.parse().map(|b| (a, b)));
    let (run_a, run_b) = match ids {
        Ok(ids) => ids,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let intentional = match intentional.map(crate::parity::load_intentional).transpose() {
        Ok(list) => list.unwrap_or_default(),
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match crate::parity::compare_runs(
        std::path::Path::new(&config.artifacts_dir),
        run_a.as_str(),
        run_b.as_str(),
        &intentional,
    ) {
        Ok(report) => {
            print!("{}", report.render());
            if report.parity {
                ExitCode::SUCCESS
            } else {
                ExitCode::from(1)
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(2)
        }
    }
}

pub(super) fn accounting_command(config_path: &std::path::Path, lane: Option<&str>) -> ExitCode {
    let config = match crate::config::ArtifactSettings::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let lane = match lane.map(str::parse::<mortsom_common::Lane>).transpose() {
        Ok(lane) => lane,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    match crate::accounting::build_accounting(std::path::Path::new(&config.artifacts_dir), lane) {
        Ok(report) => {
            print!("{}", report.render());
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::from(1)
        }
    }
}

pub(super) async fn cleanup_command(
    config_path: &std::path::Path,
    stack_dir: &std::path::Path,
    run_id: &str,
    attributions: &[String],
) -> ExitCode {
    if let Ok(run) = run_id.parse::<mortsom_common::RunId>()
        && let Some(result) =
            super::recover::cleanup_desktop(config_path, stack_dir, &run, attributions).await
    {
        return result;
    }
    if !attributions.is_empty() {
        eprintln!("--attribute-order applies only to retained desktop recovery journals");
        return ExitCode::from(2);
    }
    let config = match crate::config::MaintenanceConfig::load(config_path) {
        Ok(config) => config,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    let run_id: mortsom_common::RunId = match run_id.parse() {
        Ok(id) => id,
        Err(e) => {
            eprintln!("{e}");
            return ExitCode::from(2);
        }
    };
    if let Some(hint) = mistyped_run_hint(&config, &run_id) {
        eprintln!("{hint}");
        return ExitCode::from(2);
    }
    let env = |name: &str| std::env::var(name).ok();
    let deps = crate::cleanup_run::CleanupDeps {
        runner: &TokioRunner,
        docker: &DockerCli::default(),
        env: &env,
    };
    let summary = crate::cleanup_run::cleanup_run(&config, &run_id, &deps).await;
    print!("{}", summary.render());
    if summary.clean {
        ExitCode::SUCCESS
    } else {
        ExitCode::from(1)
    }
}

/// Refuses a run id that matches nothing on this host but is a typo away
/// from a run that does exist, naming the command the operator meant.
fn mistyped_run_hint(
    config: &crate::config::MaintenanceConfig,
    run_id: &mortsom_common::RunId,
) -> Option<String> {
    use crate::cleanup_run::lookalike::{run_ids_in, suggest_known_runs};
    let artifacts = std::path::Path::new(&config.run.artifacts_dir);
    if artifacts.join(run_id.as_str()).is_dir() {
        return None;
    }
    let recovery = RealHost.env("HOME").and_then(|home| {
        crate::recovery_store::FileJournal::directory(std::path::Path::new(&home)).ok()
    });
    let mut known = run_ids_in(artifacts);
    known.extend(recovery.iter().flat_map(|dir| run_ids_in(dir)));
    let suggestions = suggest_known_runs(run_id.as_str(), known.iter().map(String::as_str));
    let closest = suggestions.first()?;
    Some(format!(
        "run {run_id} is not known on this host; did you mean `mortsom cleanup {closest}`?"
    ))
}
