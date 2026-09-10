#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::PathBuf;

use mortsom_scenario_kit::cleanup::CleanupReport;

use super::*;

struct World {
    dir: tempfile::TempDir,
    config: Config,
}

fn world() -> World {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("mortsom.toml");
    std::fs::write(
        &path,
        format!(
            "[run]\nlane = \"local\"\nartifacts_dir = \"{}\"\n",
            dir.path().join("artifacts").display()
        ),
    )
    .unwrap();
    let config = Config::load(&path).unwrap();
    World { dir, config }
}

fn write_cleanup(world: &World, run_id: &str, unresolved: &[&str]) -> PathBuf {
    let run_dir = world.dir.path().join("artifacts").join(run_id);
    std::fs::create_dir_all(&run_dir).unwrap();
    let report = CleanupReport {
        entries: Vec::new(),
        unresolved: unresolved.iter().map(|s| (*s).to_owned()).collect(),
    };
    std::fs::write(
        run_dir.join(files::CLEANUP),
        serde_json::to_string(&report).unwrap(),
    )
    .unwrap();
    run_dir
}

#[test]
fn a_swept_run_with_no_open_order_is_clean_and_writes_its_summary() {
    let world = world();
    let run_dir = write_cleanup(&world, "run-a", &[]);

    let summary = cleanup_run(&world.config, &"run-a".parse().unwrap());

    assert!(summary.clean, "{}", summary.render());
    assert!(summary.run_dir_found && summary.cleanup_report_found);
    let written: CleanupRunSummary =
        serde_json::from_str(&std::fs::read_to_string(run_dir.join(CLEANUP_COMMAND_FILE)).unwrap())
            .unwrap();
    assert_eq!(written, summary);
    assert!(summary.render().contains("result: clean"));
}

#[test]
fn reports_unresolved_orders_instead_of_pretending() {
    let world = world();
    write_cleanup(&world, "run-c", &["order-9"]);

    let summary = cleanup_run(&world.config, &"run-c".parse().unwrap());

    assert!(!summary.clean);
    assert_eq!(summary.unresolved_orders, vec!["order-9".to_owned()]);
    let text = summary.render();
    assert!(text.contains("UNRESOLVED order order-9"));
    assert!(text.contains("result: NOT clean"));
}

#[test]
fn a_missing_run_dir_is_reported_not_hidden() {
    let world = world();

    let summary = cleanup_run(&world.config, &"never-ran".parse().unwrap());

    assert!(!summary.run_dir_found);
    assert!(!summary.cleanup_report_found);
    assert!(summary.unresolved_orders.is_empty());
    assert!(!summary.clean, "an unknown run is not reported clean");
    assert!(summary.render().contains("orders UNKNOWN"));
    assert!(!world.dir.path().join("artifacts/never-ran").exists());
}

#[test]
fn a_run_that_never_wrote_its_sweep_is_not_clean() {
    let world = world();
    std::fs::create_dir_all(world.dir.path().join("artifacts").join("run-crashed")).unwrap();

    let summary = cleanup_run(&world.config, &"run-crashed".parse().unwrap());

    assert!(summary.run_dir_found && !summary.cleanup_report_found);
    assert!(!summary.clean);
    assert!(summary.render().contains("orders UNKNOWN"));
}

#[test]
fn an_unreadable_sweep_report_is_an_unresolved_entry() {
    let world = world();
    let run_dir = world.dir.path().join("artifacts").join("run-bad");
    std::fs::create_dir_all(&run_dir).unwrap();
    std::fs::write(run_dir.join(files::CLEANUP), "not json").unwrap();

    let summary = cleanup_run(&world.config, &"run-bad".parse().unwrap());

    assert!(summary.cleanup_report_found);
    assert!(!summary.clean);
    assert!(summary.unresolved_orders[0].contains("unreadable"));
}
