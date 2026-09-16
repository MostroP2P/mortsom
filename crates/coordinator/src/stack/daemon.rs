//! Which mostrod `stack up` runs: the pinned release, an image the operator
//! names, or a Mostro branch or pull request built here at its exact commit.
use std::path::Path;
use std::time::Duration;

use mortsom_ui_driver::process::{Command, CommandRunner};

use super::compose::require;
use super::image::{self, ExpectedImage};
use super::metadata::DaemonProvenance;
use super::{DEFAULT_MOSTRO_IMAGE, StackError};
use crate::mostro_source::{self, Selector};

const PULL_TIMEOUT: Duration = Duration::from_secs(600);

/// The operator's daemon selection, validated at the CLI edge.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonSource {
    /// [`DEFAULT_MOSTRO_IMAGE`]: ordinary app review.
    PinnedImage,
    /// A published image, pulled and pinned by its official digest.
    Image(String),
    /// A branch or pull request of the Mostro repository, fetched and built
    /// here at the exact commit it resolves to.
    Ref(Selector),
}

/// Makes the selected daemon image available and says what it is. Nothing
/// cached stands in for a failed pull, fetch or build.
pub(super) async fn prepare(
    runner: &dyn CommandRunner,
    source: &DaemonSource,
    source_cache: &Path,
) -> Result<(ExpectedImage, DaemonProvenance), StackError> {
    match source {
        DaemonSource::PinnedImage => {
            let expected = pull(runner, DEFAULT_MOSTRO_IMAGE).await?;
            Ok((expected, DaemonProvenance::PinnedImage))
        }
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
