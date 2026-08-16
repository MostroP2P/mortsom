//! Where the APK under test comes from: a prebuilt file when the operator
//! points at one, otherwise a build of the configured checkout. Every
//! command that installs the app resolves it the same way, so `device
//! smoke`, `app setup` and a full run always install the same artifact.

use std::path::PathBuf;

use mortsom_android_driver::process::CommandRunner;
use mortsom_app_contract::{AppAdapter, BuildContext};

use crate::config::Config;

/// Resolves the APK to install: `mobile.apk_path` when set (it must exist),
/// otherwise the adapter builds `mobile.source_dir`.
pub async fn resolve_apk(
    config: &Config,
    adapter: &dyn AppAdapter,
    runner: &dyn CommandRunner,
) -> Result<PathBuf, String> {
    if !config.mobile.apk_path.is_empty() {
        let path = PathBuf::from(&config.mobile.apk_path);
        return if path.is_file() {
            Ok(path)
        } else {
            Err(format!("{} does not exist", path.display()))
        };
    }
    let ctx = BuildContext {
        source_dir: PathBuf::from(&config.mobile.source_dir),
        flavor: config.mobile.flavor.clone(),
        dart_defines: config.mobile.dart_defines.clone(),
    };
    adapter
        .build(&ctx, runner)
        .await
        .map(|apk| apk.path)
        .map_err(|e| format!("build failed: {e}"))
}
