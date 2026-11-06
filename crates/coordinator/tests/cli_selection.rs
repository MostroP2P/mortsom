//! A desktop run of several scenarios is a sequence of single runs, and a
//! selection that names nothing real fails before any environment is
//! touched.
#![allow(clippy::unwrap_used)]
use assert_cmd::cargo::cargo_bin_cmd;

fn run(platform: &str, selection: &[&str]) -> (i32, String, bool) {
    let root = tempfile::tempdir().unwrap();
    let mut command = cargo_bin_cmd!("mortsom");
    let output = command
        .current_dir(root.path())
        .env_clear()
        .env("HOME", root.path())
        .env("PATH", "/missing-tools")
        .args(["run", "--platform", platform])
        .args(selection)
        .output()
        .unwrap();
    (
        output.status.code().unwrap_or(-1),
        String::from_utf8_lossy(&output.stderr).into_owned()
            + &String::from_utf8_lossy(&output.stdout),
        root.path().join("artifacts").exists(),
    )
}

#[test]
fn an_unknown_scenario_or_tag_is_rejected_before_loading_any_environment() {
    for platform in ["linux", "web"] {
        for selection in [
            vec!["happy_sell", "no_such_scenario"],
            vec!["--tag", "no-such-family"],
        ] {
            let (code, output, artifacts) = run(platform, &selection);
            assert_eq!(code, 2, "{output}");
            assert!(
                output.contains("unknown scenario") || output.contains("no scenario carries tag"),
                "{output}"
            );
            assert!(!artifacts);
        }
    }
}

/// A family on the desktop is run one scenario at a time. Without a stack
/// the first run is refused, the refusal stops the family and the rest is
/// reported as not run; nothing was created.
#[test]
fn a_desktop_family_is_a_sequence_of_single_runs_that_stops_at_a_refusal() {
    for platform in ["linux", "web"] {
        for selection in [
            vec!["happy_sell", "cancel_before_taken"],
            vec!["--tag", "cancellation"],
        ] {
            let (code, output, artifacts) = run(platform, &selection);
            assert_eq!(code, 2, "{output}");
            assert!(
                !output.contains("desktop runs require exactly one scenario"),
                "{output}"
            );
            assert!(output.contains("[STOP]"), "{output}");
            assert!(output.contains("scenarios not run"), "{output}");
            assert!(!output.contains("artifacts "), "{output}");
            assert!(!artifacts);
        }
    }
}

#[test]
fn malformed_or_conflicting_source_selectors_stop_before_environment_or_fetch() {
    for platform in ["linux", "web"] {
        for selection in [
            vec!["happy_sell", "--ref", "https://github.com/other/app/pull/1"],
            vec!["happy_sell", "--ref", "main", "--source", "/app"],
            vec!["happy_sell", "--ref", "main", "--artifact", "/bundle"],
        ] {
            let (code, output, artifacts) = run(platform, &selection);
            assert_eq!(code, 2, "{output}");
            assert!(
                output.contains("invalid app selector") || output.contains("cannot be used with"),
                "{output}"
            );
            assert!(!artifacts);
        }
        let (code, output, artifacts) = run(
            platform,
            &[
                "unknown_scenario",
                "--ref",
                "https://github.com/MostroP2P/app/pull/410",
            ],
        );
        assert_eq!(code, 2, "{output}");
        assert!(output.contains("unknown scenario"), "{output}");
        assert!(!artifacts);
    }
}
