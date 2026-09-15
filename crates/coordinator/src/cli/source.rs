//! Explicit CLI overrides for configured app source selection.
use clap::Args;
use std::path::PathBuf;

/// A branch/PR or a local checkout; prebuilt artifacts bypass both config modes.
#[derive(Debug, Clone, Default, Args)]
pub struct SourceOverrides {
    /// App branch or https://github.com/MostroP2P/app/pull/123 (head commit).
    #[arg(long, value_name = "BRANCH_OR_PR_URL", conflicts_with_all = ["app_source", "app_artifact"])]
    pub app_ref: Option<crate::app_source::Selector>,
    /// Existing local app checkout, including intentional developer changes.
    #[arg(long, value_name = "PATH", conflicts_with = "app_artifact", value_parser = local_source)]
    pub app_source: Option<PathBuf>,
}

fn local_source(value: &str) -> Result<PathBuf, &'static str> {
    if value.is_empty() {
        Err("--app-source requires a non-empty local checkout path")
    } else {
        Ok(PathBuf::from(value))
    }
}

impl SourceOverrides {
    /// Typed selection passed into configuration loading before validation.
    pub fn selection(&self) -> Option<crate::config::AppSource> {
        self.app_ref
            .clone()
            .map(crate::config::AppSource::Remote)
            .or_else(|| self.app_source.clone().map(crate::config::AppSource::Local))
    }
}

#[cfg(test)]
mod tests {
    use clap::Parser;
    #[test]
    fn source_modes_conflict_and_bad_selectors_fail_before_startup() {
        for command in ["run", "doctor"] {
            for args in [
                vec!["--app-ref", "main", "--app-artifact", "/bundle"],
                vec!["--app-source", "/app", "--app-artifact", "/bundle"],
                vec!["--app-ref", "main", "--app-source", "/app"],
                vec!["--app-source", ""],
                vec!["--app-ref", "https://github.com/wrong/app/pull/2"],
            ] {
                let mut invocation = vec!["mortsom", command];
                invocation.extend(args);
                assert!(super::super::Cli::try_parse_from(invocation).is_err());
            }
            assert!(
                super::super::Cli::try_parse_from([
                    "mortsom",
                    command,
                    "--app-ref",
                    "feature/test"
                ])
                .is_ok()
            );
        }
    }
}
