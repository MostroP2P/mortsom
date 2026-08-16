//! Building the classic app for Mortsom: the test entry point plus
//! non-secret Dart defines (spec §17.1). Secrets never travel this way.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_android_driver::process::{Command, CommandRunner, run_ok};
use mortsom_app_contract::{AdapterError, Apk, BuildContext};

/// Dart entry point that arms the test environment.
pub const MORTSOM_ENTRYPOINT: &str = "lib/main_mortsom.dart";
/// Where `flutter build apk` leaves the release APK.
pub const APK_RELATIVE_PATH: &str = "build/app/outputs/flutter-apk/app-release.apk";
/// Deadline for a full build.
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(45 * 60);
/// Build flavor name understood by this adapter.
pub const MORTSOM_FLAVOR: &str = "mortsom";

/// The `flutter build apk` invocation for `ctx`, seeding `relay_url` (the
/// emulator's view of the run relay). Pure and unit tested.
pub fn build_command(flutter: &Path, ctx: &BuildContext, relay_url: &str) -> Command {
    let mut args = vec![
        "build".to_owned(),
        "apk".to_owned(),
        "-t".to_owned(),
        MORTSOM_ENTRYPOINT.to_owned(),
        "--dart-define=MORTSOM_TEST_ENV=true".to_owned(),
        format!("--dart-define=MORTSOM_RELAYS={relay_url}"),
    ];
    for define in &ctx.dart_defines {
        args.push(format!("--dart-define={define}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    Command::new(flutter, &refs)
}

/// Builds the APK in `ctx.source_dir`.
pub async fn build_apk(
    runner: &dyn CommandRunner,
    flutter: &Path,
    ctx: &BuildContext,
    relay_url: &str,
    package: &str,
) -> Result<Apk, AdapterError> {
    if ctx.flavor != MORTSOM_FLAVOR {
        return Err(AdapterError::contract(format!(
            "unknown flavor `{}`; the classic app supports `{MORTSOM_FLAVOR}` ({MORTSOM_ENTRYPOINT})",
            ctx.flavor
        )));
    }
    if ctx.dart_defines.iter().any(|d| looks_secret(d)) {
        return Err(AdapterError::contract(
            "dart_defines must not carry secrets (NWC URIs, mnemonics, keys)",
        ));
    }
    if let Err(reason) = check_relay_url(relay_url) {
        // The value itself is never echoed: it may carry the credentials
        // this check exists to stop.
        return Err(AdapterError::contract(format!(
            "relay url rejected: {reason}"
        )));
    }
    let command = build_command(flutter, ctx, relay_url);
    run_in_dir(runner, &command, &ctx.source_dir).await?;
    let path: PathBuf = ctx.source_dir.join(APK_RELATIVE_PATH);
    if !path.is_file() {
        return Err(AdapterError::infrastructure(format!(
            "flutter build finished but {} does not exist",
            path.display()
        )));
    }
    Ok(Apk {
        path,
        package: package.to_owned(),
    })
}

/// `flutter` must run inside the checkout; the runner has no cwd control, so
/// wrap through `sh -c 'cd <dir> && exec "$@"'`.
async fn run_in_dir(
    runner: &dyn CommandRunner,
    command: &Command,
    dir: &Path,
) -> Result<(), AdapterError> {
    let mut args = vec![
        "-c".to_owned(),
        "cd \"$0\" && exec \"$@\"".to_owned(),
        dir.display().to_string(),
        command.program.display().to_string(),
    ];
    args.extend(command.args.iter().cloned());
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    let shell = Command::new("sh", &refs);
    run_ok(runner, &shell, BUILD_TIMEOUT)
        .await
        .map(|_| ())
        .map_err(|e| AdapterError::infrastructure(format!("flutter build failed: {e}")))
}

/// The relay url is interpolated into a dart-define, so it is baked into the
/// APK and into build logs. Only a credential-free websocket url qualifies.
fn check_relay_url(url: &str) -> Result<(), &'static str> {
    let Some(rest) = url
        .strip_prefix("ws://")
        .or_else(|| url.strip_prefix("wss://"))
    else {
        return Err("only ws:// and wss:// urls are accepted");
    };
    let authority = rest.split(['/', '?', '#']).next().unwrap_or(rest);
    if authority.is_empty() {
        return Err("the url has no host");
    }
    if authority.contains('@') {
        return Err("credentials in the url would be baked into the build");
    }
    if looks_secret(url) {
        return Err("the url looks like it carries a secret");
    }
    Ok(())
}

fn looks_secret(define: &str) -> bool {
    let lower = define.to_ascii_lowercase();
    lower.contains("nostr+walletconnect://")
        || lower.contains("nsec1")
        || lower.contains("mnemonic")
        || lower.contains("secret")
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use mortsom_android_driver::process::tests_support::FakeRunner;

    fn ctx(dir: &Path) -> BuildContext {
        BuildContext {
            source_dir: dir.to_path_buf(),
            flavor: MORTSOM_FLAVOR.into(),
            dart_defines: vec!["MOSTRO_PUB_KEY=abc".into()],
        }
    }

    #[test]
    fn build_command_uses_the_test_entry_point_and_defines() {
        let cmd = build_command(
            Path::new("/bin/flutter"),
            &ctx(Path::new("/src")),
            "ws://10.0.2.2:7000",
        );
        assert_eq!(cmd.args[..4], ["build", "apk", "-t", MORTSOM_ENTRYPOINT]);
        assert!(
            cmd.args
                .contains(&"--dart-define=MORTSOM_TEST_ENV=true".to_owned())
        );
        assert!(
            cmd.args
                .contains(&"--dart-define=MORTSOM_RELAYS=ws://10.0.2.2:7000".to_owned())
        );
        assert!(
            cmd.args
                .contains(&"--dart-define=MOSTRO_PUB_KEY=abc".to_owned())
        );
    }

    #[tokio::test]
    async fn build_runs_in_the_checkout_and_returns_the_apk() {
        let dir = tempfile::tempdir().unwrap();
        let apk = dir.path().join(APK_RELATIVE_PATH);
        std::fs::create_dir_all(apk.parent().unwrap()).unwrap();
        std::fs::write(&apk, b"apk").unwrap();
        let runner = FakeRunner::default();

        let built = build_apk(
            &runner,
            Path::new("/bin/flutter"),
            &ctx(dir.path()),
            "ws://10.0.2.2:7000",
            "network.mostro.app",
        )
        .await
        .unwrap();

        assert_eq!(built.path, apk);
        assert_eq!(built.package, "network.mostro.app");
        let call = &runner.calls()[0];
        assert!(call.starts_with("sh -c"), "{call}");
        assert!(call.contains(&dir.path().display().to_string()));
        assert!(call.contains("build apk -t lib/main_mortsom.dart"));
    }

    #[tokio::test]
    async fn build_refuses_secrets_and_unknown_flavors_and_missing_output() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();
        let mut secret = ctx(dir.path());
        secret
            .dart_defines
            .push("NWC=nostr+walletconnect://x".into());
        assert!(
            build_apk(&runner, Path::new("f"), &secret, "ws://r", "p")
                .await
                .is_err()
        );
        let mut flavor = ctx(dir.path());
        flavor.flavor = "prod".into();
        assert!(
            build_apk(&runner, Path::new("f"), &flavor, "ws://r", "p")
                .await
                .is_err()
        );
        let err = build_apk(&runner, Path::new("f"), &ctx(dir.path()), "ws://r", "p")
            .await
            .unwrap_err();
        assert!(err.message.contains("does not exist"));
        assert!(runner.calls().len() == 1, "only the valid build ran");
    }

    #[tokio::test]
    async fn build_refuses_a_relay_url_that_carries_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();

        let err = build_apk(
            &runner,
            Path::new("/bin/flutter"),
            &ctx(dir.path()),
            "wss://user:hunter2@relay.example",
            "network.mostro.app",
        )
        .await
        .unwrap_err();

        assert!(err.message.contains("relay"), "{err}");
        // The rejected value never reaches the message or the process.
        assert!(!format!("{err}").contains("hunter2"), "{err}");
        assert!(runner.calls().is_empty(), "nothing may be built");
    }
}
