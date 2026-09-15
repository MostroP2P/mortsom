//! One immutable app build for every scenario selected in one CLI invocation.
use crate::run::DesktopOptions;
use crate::stack::StackFingerprint;

#[derive(Default)]
pub(super) struct InvocationBuild {
    prepared: Option<(DesktopOptions, StackFingerprint)>,
}

impl InvocationBuild {
    /// Calls the builder only once and refuses a changed stack before reuse.
    pub(super) async fn get_or_prepare<F, Fut>(
        &mut self,
        stack: StackFingerprint,
        build: F,
    ) -> Result<DesktopOptions, String>
    where
        F: FnOnce() -> Fut,
        Fut: Future<Output = Result<DesktopOptions, String>>,
    {
        if let Some((options, original)) = &self.prepared {
            if original != &stack {
                return Err(
                    "stack changed during the scenario invocation; refusing to reuse the app build"
                        .into(),
                );
            }
            return Ok(options.clone());
        }
        let options = build().await?;
        self.prepared = Some((options.clone(), stack));
        Ok(options)
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn options() -> DesktopOptions {
        let root = tempfile::tempdir().unwrap();
        std::fs::write(root.path().join("index.html"), b"original build").unwrap();
        let bundle = Arc::new(crate::fingerprint::CapturedBundle::capture(root.path()).unwrap());
        DesktopOptions {
            artifact: bundle.root().into(),
            bundle,
            recovery: None,
            resume: false,
            platform: mortsom_desktop_driver::Platform::Web,
            provenance: crate::provenance::BuildProvenance::prebuilt(),
            xvfb: "Xvfb".into(),
            chromedriver: "chromedriver".into(),
            browser: "chromium".into(),
        }
    }

    fn stack(digest: &str) -> StackFingerprint {
        StackFingerprint {
            algorithm: "sha256-mortsom-stack-v1".into(),
            digest: digest.into(),
        }
    }

    #[tokio::test]
    async fn family_reuses_the_exact_bundle_without_fetching_or_building_again() {
        let mut invocation = InvocationBuild::default();
        let first = invocation
            .get_or_prepare(stack("same"), || async { Ok(options()) })
            .await
            .unwrap();
        let second = invocation
            .get_or_prepare(stack("same"), || async {
                panic!("a moving remote or edited local source must not trigger another build")
            })
            .await
            .unwrap();
        assert!(Arc::ptr_eq(&first.bundle, &second.bundle));
        assert_eq!(first.provenance, second.provenance);
        assert!(first.recovery.is_none() && second.recovery.is_none());
        let result = invocation
            .get_or_prepare(stack("changed"), || async {
                panic!("do not rebuild against a changed stack")
            })
            .await;
        assert!(result.unwrap_err().contains("stack changed"));
    }

    #[tokio::test]
    async fn failed_preparation_is_not_cached_as_a_success() {
        let mut invocation = InvocationBuild::default();
        let result = invocation
            .get_or_prepare(stack("same"), || async { Err("fetch failed".into()) })
            .await;
        assert_eq!(result.unwrap_err(), "fetch failed");
        assert!(invocation.prepared.is_none());
    }
}
