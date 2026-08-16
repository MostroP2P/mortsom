//! Building the v2 app under test with `flutter build apk`. The v2 project
//! has no product flavors; the Mortsom entry point and dart-defines select
//! the test environment (spec §17.1). Secrets never enter dart-defines.

use std::path::{Path, PathBuf};
use std::time::Duration;

use mortsom_android_driver::process::{Command, CommandRunner, run_ok};
use mortsom_app_contract::{AdapterError, Apk, BuildContext};

/// Entry point that arms the test environment.
pub const MORTSOM_ENTRYPOINT: &str = "lib/main_mortsom.dart";
/// Release APK path relative to the checkout.
pub const APK_RELATIVE_PATH: &str = "build/app/outputs/flutter-apk/app-release.apk";
/// Debug APK path relative to the checkout.
pub const DEBUG_APK_RELATIVE_PATH: &str = "build/app/outputs/flutter-apk/app-debug.apk";
/// Deadline for a Flutter build (Rust core included).
pub const BUILD_TIMEOUT: Duration = Duration::from_secs(60 * 60);
/// Accepted `mobile.flavor` values: v2 has no flavors, `debug` picks a
/// debug build.
pub const FLAVORS: [&str; 3] = ["", "mortsom", "debug"];

/// The `flutter build apk` command line.
pub fn build_command(flutter: &Path, ctx: &BuildContext, relay_url: &str) -> Command {
    let mut args = vec![
        "build".to_owned(),
        "apk".to_owned(),
        "-t".to_owned(),
        MORTSOM_ENTRYPOINT.to_owned(),
        "--dart-define=MORTSOM_TEST_ENV=true".to_owned(),
        format!("--dart-define=MORTSOM_RELAYS={relay_url}"),
    ];
    if ctx.flavor == "debug" {
        args.push("--debug".to_owned());
    }
    for define in &ctx.dart_defines {
        args.push(format!("--dart-define={define}"));
    }
    let refs: Vec<&str> = args.iter().map(String::as_str).collect();
    Command::new(flutter, &refs)
}

/// Builds the APK in `ctx.source_dir` and returns it.
pub async fn build_apk(
    runner: &dyn CommandRunner,
    flutter: &Path,
    ctx: &BuildContext,
    relay_url: &str,
    package: &str,
) -> Result<Apk, AdapterError> {
    if !FLAVORS.contains(&ctx.flavor.as_str()) {
        return Err(AdapterError::contract(format!(
            "unknown flavor `{}`; the v2 app has none (use empty, `mortsom` or `debug`)",
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
    let relative = if ctx.flavor == "debug" {
        DEBUG_APK_RELATIVE_PATH
    } else {
        APK_RELATIVE_PATH
    };
    let path: PathBuf = ctx.source_dir.join(relative);
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

    fn ctx(dir: &Path, flavor: &str) -> BuildContext {
        BuildContext {
            source_dir: dir.to_path_buf(),
            flavor: flavor.into(),
            dart_defines: vec!["MOSTRO_PUB_KEY=abc".into()],
        }
    }

    #[test]
    fn build_command_arms_the_test_environment() {
        let cmd = build_command(
            Path::new("/bin/flutter"),
            &ctx(Path::new("/src"), "debug"),
            "ws://10.0.2.2:7000",
        );
        assert_eq!(cmd.args[..4], ["build", "apk", "-t", MORTSOM_ENTRYPOINT]);
        assert!(cmd.args.contains(&"--debug".to_owned()));
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
    async fn build_returns_the_apk_and_refuses_secrets_and_flavors() {
        let dir = tempfile::tempdir().unwrap();
        let apk = dir.path().join(APK_RELATIVE_PATH);
        std::fs::create_dir_all(apk.parent().unwrap()).unwrap();
        std::fs::write(&apk, b"apk").unwrap();
        let runner = FakeRunner::default();

        let built = build_apk(
            &runner,
            Path::new("/bin/flutter"),
            &ctx(dir.path(), ""),
            "ws://10.0.2.2:7000",
            "foundation.mostro.app",
        )
        .await
        .unwrap();
        assert_eq!(built.path, apk);
        assert_eq!(built.package, "foundation.mostro.app");
        assert!(runner.calls()[0].contains("build apk -t lib/main_mortsom.dart"));

        let err = build_apk(
            &runner,
            Path::new("/bin/flutter"),
            &ctx(dir.path(), "prod"),
            "ws://r",
            "p",
        )
        .await
        .unwrap_err();
        assert!(err.message.contains("unknown flavor"));

        let mut secret = ctx(dir.path(), "");
        secret
            .dart_defines
            .push("NWC=nostr+walletconnect://x".into());
        let err = build_apk(&runner, Path::new("/bin/flutter"), &secret, "ws://r", "p")
            .await
            .unwrap_err();
        assert!(err.message.contains("secrets"));
    }

    #[tokio::test]
    async fn build_refuses_a_relay_url_that_carries_credentials() {
        let dir = tempfile::tempdir().unwrap();
        let runner = FakeRunner::default();

        let err = build_apk(
            &runner,
            Path::new("/bin/flutter"),
            &ctx(dir.path(), ""),
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
