use std::path::Path;
use std::time::Instant;

use rustix::process::{kill_process, test_kill_process_group};

use super::*;

/// How long a helper waits for the shell to have done something, or for a
/// killed process to be gone.
const PATIENCE: Duration = Duration::from_secs(2);
/// A deadline that leaves the shell time to start and take its first steps
/// on a loaded machine before it passes.
const SHELL_DEADLINE: Duration = Duration::from_secs(2);
/// A background descendant that keeps both output pipes.
const HOLDS_BOTH: &str = "";
/// A background descendant that keeps neither output pipe.
const HOLDS_NOTHING: &str = ">/dev/null 2>&1";
/// A background descendant that keeps standard output only.
const HOLDS_STDOUT: &str = "2>/dev/null";

fn shell(script: &str, env: &[(&str, &str)], stdin: Option<&str>) -> ProcessRequest {
    ProcessRequest {
        program: PathBuf::from("/bin/sh"),
        args: vec!["-c".into(), script.into()],
        env: env
            .iter()
            .map(|(k, v)| ((*k).to_owned(), (*v).to_owned()))
            .collect(),
        current_dir: std::env::temp_dir(),
        stdin: stdin.map(Secret::new),
    }
}

/// A script that starts a background `sleep` descendant with the output
/// redirections `holds`, records its pid in `pid_file` and then does
/// `rest`.
fn with_descendant(pid_file: &Path, holds: &str, rest: &str) -> String {
    format!(
        "sleep 10 {holds} & echo $! > '{}'; {rest}",
        pid_file.display()
    )
}

/// The pid the script wrote to `pid_file`, once it has written the whole
/// line: the file may not exist yet, or be created but still empty.
async fn pid_in(pid_file: &Path) -> u32 {
    let deadline = Instant::now() + PATIENCE;
    loop {
        let written = std::fs::read_to_string(pid_file).unwrap_or_default();
        let pid = written
            .strip_suffix('\n')
            .and_then(|line| line.trim().parse::<u32>().ok())
            .filter(|pid| *pid > 0);
        if let Some(pid) = pid {
            return pid;
        }
        assert!(
            Instant::now() < deadline,
            "the script wrote no pid to {} within {PATIENCE:?} (found {written:?})",
            pid_file.display()
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Whether the process `pid` is gone within [`PATIENCE`]. A killed process
/// is reparented and may linger as a zombie until its new parent reaps it:
/// that counts as gone. A survivor is killed before this says so, so a
/// failing test leaks no process.
async fn is_gone(pid: u32) -> bool {
    let stat = PathBuf::from(format!("/proc/{pid}/stat"));
    let deadline = Instant::now() + PATIENCE;
    loop {
        let Ok(text) = std::fs::read_to_string(&stat) else {
            return true;
        };
        if process_state(&text) == Some('Z') {
            return true;
        }
        if Instant::now() >= deadline {
            kill_sleep(pid);
            return false;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

/// Kills `pid` if it is still one of the tests' `sleep` processes, and
/// leaves a recycled pid alone.
fn kill_sleep(pid: u32) {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).unwrap_or_default();
    let pid = i32::try_from(pid).ok().and_then(Pid::from_raw);
    if let (true, Some(pid)) = (stat.contains("(sleep)"), pid) {
        kill_process(pid, Signal::KILL).ok();
    }
}

/// The fields of a `/proc/<pid>/stat` line after the parenthesised command
/// name, which may itself hold spaces and parentheses: the state, the
/// parent, the process group and so on.
fn fields_after_name(stat: &str) -> Vec<&str> {
    stat.rsplit_once(')')
        .map(|(_, rest)| rest.split_whitespace().collect())
        .unwrap_or_default()
}

/// The state field of a `/proc/<pid>/stat` line.
fn process_state(stat: &str) -> Option<char> {
    fields_after_name(stat).first()?.chars().next()
}

/// The process group field of a `/proc/<pid>/stat` line.
fn process_group(stat: &str) -> Option<u32> {
    fields_after_name(stat).get(2)?.parse().ok()
}

#[test]
fn the_stat_fields_follow_the_last_parenthesis() {
    assert_eq!(process_state("42 (a) b) Z 1 42"), Some('Z'));
    assert_eq!(process_state("42 (sleep) S 1 7"), Some('S'));
    assert_eq!(process_group("42 (a) b) Z 1 7 7"), Some(7));
    assert_eq!(process_state("garbage"), None);
    assert_eq!(process_group("42 (sleep) S"), None);
}

fn exited_with(code: i32) -> ExitStatus {
    std::os::unix::process::ExitStatusExt::from_raw(code << 8)
}

fn killed_by(signal: i32) -> ExitStatus {
    std::os::unix::process::ExitStatusExt::from_raw(signal)
}

#[test]
fn a_leader_that_exited_and_whose_output_ended_is_an_output() {
    let collected = Collected::Whole {
        stdout: "{}".into(),
        stderr: "warn".into(),
    };

    let output = outcome(exited_with(3), collected, Duration::from_secs(1)).unwrap();

    assert_eq!(
        output,
        ProcessOutput {
            code: Some(3),
            stdout: "{}".into(),
            stderr: "warn".into(),
        }
    );
}

#[test]
fn an_output_that_failed_to_read_keeps_the_standard_error_read_so_far() {
    let failure = ProcessFailure::Io("connection reset".into());
    let collected = Collected::Failed {
        failure: failure.clone(),
        stderr: "partial".into(),
    };

    let failed = outcome(exited_with(0), collected, Duration::from_secs(1)).unwrap_err();

    assert_eq!(
        failed,
        ProcessFailed {
            failure,
            stderr: "partial".into(),
        }
    );
}

#[test]
fn an_output_still_held_at_the_deadline_is_a_timeout_that_keeps_standard_error() {
    let collected = Collected::Late {
        stderr: "why".into(),
    };

    let failed = outcome(exited_with(0), collected, Duration::from_secs(7)).unwrap_err();

    assert_eq!(
        failed.failure,
        ProcessFailure::Timeout(Duration::from_secs(7))
    );
    assert_eq!(failed.stderr, "why");
}

#[test]
fn a_leader_a_signal_ended_is_signaled_whatever_became_of_its_output() {
    let collections = [
        Collected::Whole {
            stdout: "{}".into(),
            stderr: "bye".into(),
        },
        Collected::Failed {
            failure: ProcessFailure::Io("reset".into()),
            stderr: "bye".into(),
        },
        Collected::Late {
            stderr: "bye".into(),
        },
    ];

    for collected in collections {
        let failed = outcome(killed_by(9), collected, Duration::from_secs(1)).unwrap_err();

        assert_eq!(failed.failure, ProcessFailure::Signaled);
        assert_eq!(failed.stderr, "bye");
    }
}

#[tokio::test]
async fn the_secret_reaches_standard_input_and_the_exit_code_is_kept() {
    let request = shell("cat; echo oops >&2; exit 3", &[], Some("lnbcrt1secret"));

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(output.code, Some(3));
    assert_eq!(output.stdout, "lnbcrt1secret\n");
    assert_eq!(output.stderr.trim(), "oops");
}

#[tokio::test]
async fn the_child_inherits_no_environment() {
    let request = shell(
        "echo \"${HOME:-unset} ${MCLI_DIR:-unset}\"",
        &[("MCLI_DIR", "/private")],
        None,
    );

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(output.stdout.trim(), "unset /private");
}

/// The whole group is what ends with the invocation: the child must lead
/// one of its own, never share the coordinator's.
#[tokio::test]
async fn the_child_leads_its_own_process_group() {
    let request = shell(
        "read -r stat < /proc/self/stat; echo \"$$\"; echo \"$stat\"",
        &[],
        None,
    );

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    let (pid, stat) = output.stdout.split_once('\n').unwrap();
    let pid: u32 = pid.parse().unwrap();
    assert_eq!(process_group(stat), Some(pid), "{stat}");
}

#[tokio::test]
async fn without_a_secret_standard_input_is_already_at_its_end() {
    let request = shell("cat; echo done", &[], None);

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(output.stdout, "done\n");
}

#[tokio::test]
async fn a_child_that_never_reads_its_secret_is_not_a_pipe_failure() {
    let request = shell("exec 0<&-; echo refused; exit 2", &[], Some("x"));

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(output.code, Some(2));
}

#[tokio::test]
async fn a_child_past_its_deadline_is_a_timeout() {
    let request = shell("exec sleep 10", &[], None);

    let failure = TokioCliProcess
        .run(&request, Duration::from_millis(200))
        .await
        .unwrap_err();

    assert_eq!(
        failure.failure,
        ProcessFailure::Timeout(Duration::from_millis(200))
    );
}

#[tokio::test]
async fn a_child_ended_by_a_signal_is_no_output() {
    let request = shell("kill -9 $$", &[], None);

    let failure = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap_err();

    assert_eq!(failure.failure, ProcessFailure::Signaled);
}

#[tokio::test]
async fn a_timed_out_child_is_reaped_when_the_timeout_is_reported() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!("echo $$ > '{}'; exec sleep 10", pid_file.display());
    let request = shell(&script, &[], None);

    let failure = TokioCliProcess
        .run(&request, SHELL_DEADLINE)
        .await
        .unwrap_err();

    assert_eq!(failure.failure, ProcessFailure::Timeout(SHELL_DEADLINE));
    let pid = pid_in(&pid_file).await;
    let reaped = !Path::new(&format!("/proc/{pid}")).exists();
    if !reaped {
        kill_sleep(pid);
    }
    assert!(
        reaped,
        "the timed-out child is still there, if only a zombie"
    );
}

#[tokio::test]
async fn a_missing_executable_is_a_spawn_failure() {
    let mut request = shell("true", &[], None);
    request.program = PathBuf::from("/nonexistent/mostro-cli");

    let failure = TokioCliProcess
        .run(&request, Duration::from_secs(1))
        .await
        .unwrap_err();

    assert!(
        matches!(failure.failure, ProcessFailure::Spawn(_)),
        "{failure:?}"
    );
}

/// A child that hangs or is killed may have said why on standard error:
/// the failure carries it, so the run's evidence keeps it.
#[tokio::test]
async fn a_failed_child_keeps_what_it_wrote_to_standard_error() {
    let hung = shell("echo stuck >&2; exec sleep 10", &[], None);
    let killed = shell("echo bye >&2; kill -9 $$", &[], None);

    let timed_out = TokioCliProcess
        .run(&hung, SHELL_DEADLINE)
        .await
        .unwrap_err();
    let signaled = TokioCliProcess
        .run(&killed, Duration::from_secs(10))
        .await
        .unwrap_err();

    assert_eq!(timed_out.failure, ProcessFailure::Timeout(SHELL_DEADLINE));
    assert_eq!(timed_out.stderr.trim(), "stuck");
    assert_eq!(signaled.failure, ProcessFailure::Signaled);
    assert_eq!(signaled.stderr.trim(), "bye");
}

/// A one-shot CLI leaves nothing behind: a descendant that stayed in its
/// group, even one that keeps no output open, ends when the leader exits
/// instead of keeping the actor's directory in use.
#[tokio::test]
async fn a_descendant_without_the_output_does_not_outlive_a_clean_exit() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let request = shell(
        &with_descendant(&pid_file, HOLDS_NOTHING, "exit 0"),
        &[],
        None,
    );

    let output = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap();

    assert_eq!(output.code, Some(0));
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
}

#[tokio::test]
async fn a_descendant_without_the_output_does_not_outlive_a_leader_a_signal_ended() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = with_descendant(&pid_file, HOLDS_NOTHING, "kill -9 $$");
    let request = shell(&script, &[], None);

    let failed = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap_err();

    assert_eq!(failed.failure, ProcessFailure::Signaled);
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
}

#[tokio::test]
async fn a_descendant_holding_only_standard_output_does_not_outlive_a_leader_a_signal_ended() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!(
        "echo bye >&2; {}",
        with_descendant(&pid_file, HOLDS_STDOUT, "kill -9 $$")
    );
    let request = shell(&script, &[], None);
    let started = Instant::now();

    let failed = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap_err();

    let elapsed = started.elapsed();
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(failed.failure, ProcessFailure::Signaled);
    assert_eq!(failed.stderr.trim(), "bye");
}

#[tokio::test]
async fn a_child_ended_by_a_signal_takes_a_descendant_holding_its_output_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = with_descendant(&pid_file, HOLDS_BOTH, "kill -9 $$");
    let request = shell(&script, &[], None);

    let failed = TokioCliProcess
        .run(&request, Duration::from_secs(10))
        .await
        .unwrap_err();

    assert_eq!(failed.failure, ProcessFailure::Signaled);
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
}

/// A wrapper that exits but leaves a background process holding its output
/// open neither holds the invocation up nor leaves the process behind: the
/// group ends with the leader, which closes the output.
#[tokio::test]
async fn a_descendant_holding_the_output_open_ends_with_the_leader() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = format!(
        "echo why >&2; {}",
        with_descendant(&pid_file, HOLDS_BOTH, "exit 0")
    );
    let request = shell(&script, &[], None);
    let started = Instant::now();

    let outcome = TokioCliProcess.run(&request, Duration::from_secs(5)).await;

    let elapsed = started.elapsed();
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
    let output = outcome.unwrap();
    assert!(elapsed < Duration::from_secs(4), "{elapsed:?}");
    assert_eq!(output.code, Some(0));
    assert_eq!(output.stderr.trim(), "why");
}

/// A holder that left the group is beyond the group's end: the deadline
/// alone bounds the wait for the output it keeps open, and what the child
/// wrote to standard error is kept.
#[tokio::test]
async fn a_holder_that_left_the_group_cannot_outlast_the_deadline() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let pid_path = pid_file.display();
    // The leader exits only once the holder has its own session.
    let script = format!(
        "echo why >&2; setsid sh -c 'echo $$ > \"{pid_path}\"; exec sleep 10' & \
         until [ -s '{pid_path}' ]; do sleep 0.05; done; exit 0"
    );
    let request = shell(&script, &[], None);
    let started = Instant::now();

    let failed = TokioCliProcess
        .run(&request, SHELL_DEADLINE)
        .await
        .unwrap_err();

    let elapsed = started.elapsed();
    // Not the run's to end, but the test's.
    kill_sleep(pid_in(&pid_file).await);
    assert!(elapsed < SHELL_DEADLINE + PATIENCE, "{elapsed:?}");
    assert_eq!(failed.failure, ProcessFailure::Timeout(SHELL_DEADLINE));
    assert_eq!(failed.stderr.trim(), "why");
}

/// A leader a signal ended gave no answer: a holder that escaped its group
/// buys it the grace for standard error, not the rest of the deadline.
#[tokio::test]
async fn a_leader_a_signal_ended_waits_only_the_grace_for_an_escaped_holder() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let pid_path = pid_file.display();
    let script = format!(
        "echo why >&2; setsid sh -c 'echo $$ > \"{pid_path}\"; exec sleep 10' & \
         until [ -s '{pid_path}' ]; do sleep 0.05; done; kill -9 $$"
    );
    let request = shell(&script, &[], None);
    let deadline = Duration::from_secs(8);
    let started = Instant::now();

    let failed = TokioCliProcess.run(&request, deadline).await.unwrap_err();

    let elapsed = started.elapsed();
    // Not the run's to end, but the test's.
    kill_sleep(pid_in(&pid_file).await);
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    assert_eq!(failed.failure, ProcessFailure::Signaled);
    assert_eq!(failed.stderr.trim(), "why");
}

/// A surviving descendant would keep the actor's private directory in use
/// past the deadline that promised the invocation was over.
#[tokio::test]
async fn a_timed_out_child_takes_its_descendants_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = with_descendant(&pid_file, HOLDS_BOTH, "exec sleep 10");
    let request = shell(&script, &[], None);

    let failed = TokioCliProcess
        .run(&request, SHELL_DEADLINE)
        .await
        .unwrap_err();

    assert_eq!(failed.failure, ProcessFailure::Timeout(SHELL_DEADLINE));
    let pid = pid_in(&pid_file).await;
    assert!(is_gone(pid).await, "the descendant outlived the run");
}

/// An invocation abandoned by its caller ends the same way a timed-out one
/// does. The run is dropped as soon as the descendant exists.
#[tokio::test]
async fn a_cancelled_run_takes_the_descendants_with_it() {
    let dir = tempfile::tempdir().unwrap();
    let pid_file = dir.path().join("pid");
    let script = with_descendant(&pid_file, HOLDS_BOTH, "exec sleep 10");
    let request = shell(&script, &[], None);

    let pid = tokio::select! {
        ended = TokioCliProcess.run(&request, Duration::from_secs(30)) => {
            panic!("the run ended before it was cancelled: {ended:?}")
        }
        pid = pid_in(&pid_file) => pid,
    };

    assert!(is_gone(pid).await, "the descendant outlived the run");
}

/// Why the group may be signalled while the leader is unreaped: an exited
/// leader stays a member of its group until it is reaped, so the group id
/// cannot belong to anyone else and ESRCH before the reap is unexpected.
#[tokio::test]
async fn an_exited_leader_keeps_its_group_until_it_is_reaped() {
    let mut child = command(&shell("exit 0", &[], None)).spawn().unwrap();
    let pid = Pid::from_raw(i32::try_from(child.id().unwrap()).unwrap()).unwrap();
    let options = WaitIdOptions::EXITED | WaitIdOptions::NOHANG | WaitIdOptions::NOWAIT;
    let deadline = Instant::now() + PATIENCE;
    while waitid(WaitId::Pid(pid), options).unwrap().is_none() {
        assert!(Instant::now() < deadline, "the leader never exited");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let before = test_kill_process_group(pid);
    child.wait().await.unwrap();
    let after = test_kill_process_group(pid);

    assert_eq!(before, Ok(()));
    assert_eq!(after, Err(Errno::SRCH));
}
