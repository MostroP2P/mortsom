//! Which mostrod `stack up` runs: Mostro's `main`, or another branch or pull
//! request, built here at its exact commit, or an image the operator names.
use std::path::Path;
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner};

use super::StackError;
use super::compose::require;
use super::image::{self, ExpectedImage};
use super::metadata::DaemonProvenance;
use crate::mostro_source::{self, Selector};

const PULL_TIMEOUT: Duration = Duration::from_secs(600);

/// The operator's daemon selection, validated at the CLI edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonSource {
    /// A published image, pulled and pinned by its official digest.
    Image(String),
    /// A branch or pull request of the Mostro repository, fetched and built
    /// here at the exact commit it resolves to.
    Ref(Selector),
}

impl DaemonSource {
    /// The daemon `stack up --mostro-image`/`--mostro-ref` select: the named
    /// image or ref, and Mostro's `main` when neither is given, so the stack
    /// tests the daemon as it is about to ship. The CLI never passes both.
    pub fn from_options(image: Option<String>, git_ref: Option<Selector>) -> Self {
        match (image, git_ref) {
            (_, Some(selector)) => Self::Ref(selector),
            (Some(image), None) => Self::Image(image),
            (None, None) => Self::Ref(Selector::main()),
        }
    }
}

/// Makes the selected daemon image available and says what it is. Nothing
/// cached stands in for a failed pull, fetch or build.
pub(super) async fn prepare(
    runner: &dyn CommandRunner,
    source: &DaemonSource,
    source_cache: &Path,
) -> Result<(ExpectedImage, DaemonProvenance), StackError> {
    match source {
        DaemonSource::Image(requested) => {
            let expected = pull(runner, requested).await?;
            let provenance = DaemonProvenance::Image {
                requested: requested.clone(),
            };
            Ok((expected, provenance))
        }
        DaemonSource::Ref(selector) => {
            tracing::info!(selector = %selector, "fetching the daemon source");
            // The checkout is disposable: dropping it at the end of this
            // arm removes it whether or not the build succeeded.
            let checkout = crate::git_source::prepare_from(
                source_cache,
                mostro_source::REPOSITORY,
                selector,
                runner,
            )
            .await?;
            let expected = image::build(runner, &checkout).await?;
            Ok((
                expected,
                DaemonProvenance::GitRef(checkout.selection.clone()),
            ))
        }
    }
}

async fn pull(runner: &dyn CommandRunner, reference: &str) -> Result<ExpectedImage, StackError> {
    // A failed pull is an error, never permission to use a cached image.
    tracing::info!(image = reference, "pulling the daemon image");
    require(
        runner,
        &Command::new("docker", &["pull", reference]),
        PULL_TIMEOUT,
    )
    .await?;
    image::resolve(runner, reference).await
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn naming_no_source_builds_mostro_main() {
        assert_eq!(
            DaemonSource::from_options(None, None),
            DaemonSource::Ref(Selector::main())
        );
    }

    #[test]
    fn a_named_image_or_ref_is_what_runs() {
        // Arrange
        let pr = Selector::parse("https://github.com/MostroP2P/mostro/pull/963").unwrap();
        let image = "mostrop2p/mostro:v0.19.3".to_owned();

        // Act / Assert
        assert_eq!(
            DaemonSource::from_options(None, Some(pr.clone())),
            DaemonSource::Ref(pr)
        );
        assert_eq!(
            DaemonSource::from_options(Some(image.clone()), None),
            DaemonSource::Image(image)
        );
    }
}
