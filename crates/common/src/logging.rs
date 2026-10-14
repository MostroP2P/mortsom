//! Structured logging: `tracing` to stderr plus a redacted per-run log file
//! that can be attached once the run directory exists.

use std::fs::{File, OpenOptions};
use std::io::{self, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use tracing_subscriber::EnvFilter;
use tracing_subscriber::Layer;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;

use crate::redact::Redactor;

/// Default filter for `run.log` when `RUST_LOG` is unset.
pub const DEFAULT_FILTER: &str = "mortsom=info";
/// Default filter for the terminal when `RUST_LOG` is unset. The scenario
/// narrative (every timeline step, payment settlement and solver message, and
/// the run's own bookkeeping) goes to `run.log` and `timeline.md` only: the
/// operator reads the run's report, not its trace. The terminal keeps the
/// progress of long operations (client builds, `stack up`) and every warning.
pub const DEFAULT_TERMINAL_FILTER: &str = "mortsom=info,mortsom_scenario_kit=warn,\
    mortsom_environment::lnd=warn,mortsom_solver=warn,\
    mortsom_coordinator::run=warn,mortsom_coordinator::report=warn";
/// File name of the per-run log inside the artifact directory.
pub const RUN_LOG_FILE: &str = "run.log";

static POISON_REPORTED: AtomicBool = AtomicBool::new(false);

fn lock_recovering<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|poisoned| {
        if !POISON_REPORTED.swap(true, Ordering::Relaxed) {
            eprintln!("mortsom: log writer mutex was poisoned; continuing with its contents");
        }
        poisoned.into_inner()
    })
}

/// A writer that redacts every complete line before it reaches the shared
/// sink. Each instance owns its partial-line buffer, so concurrent writers
/// never interleave fragments; the buffer is flushed on drop.
pub struct RedactingWriter<W: Write> {
    inner: Arc<Mutex<W>>,
    redactor: Arc<Redactor>,
    pending: Vec<u8>,
}

impl<W: Write> RedactingWriter<W> {
    /// Wraps `inner` so that every line passes through `redactor`.
    pub fn new(inner: W, redactor: Redactor) -> Self {
        Self {
            inner: Arc::new(Mutex::new(inner)),
            redactor: Arc::new(redactor),
            pending: Vec::new(),
        }
    }

    /// A new writer sharing the sink and redactor but with its own buffer.
    pub fn handle(&self) -> Self {
        Self {
            inner: Arc::clone(&self.inner),
            redactor: Arc::clone(&self.redactor),
            pending: Vec::new(),
        }
    }

    fn write_line(&self, line: &[u8]) -> io::Result<()> {
        let redacted = self.redactor.redact(&String::from_utf8_lossy(line));
        let mut inner = lock_recovering(&self.inner);
        inner.write_all(redacted.as_bytes())?;
        inner.write_all(b"\n")
    }
}

impl<W: Write> Write for RedactingWriter<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.pending.extend_from_slice(buf);
        while let Some(pos) = self.pending.iter().position(|b| *b == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=pos).collect();
            self.write_line(&line[..line.len() - 1])?;
        }
        Ok(buf.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        if !self.pending.is_empty() {
            let line = std::mem::take(&mut self.pending);
            self.write_line(&line)?;
        }
        lock_recovering(&self.inner).flush()
    }
}

impl<W: Write> Drop for RedactingWriter<W> {
    fn drop(&mut self) {
        // Best effort: an unterminated tail must not be lost.
        let _ = self.flush();
    }
}

/// Global sink for the per-run log file. Writes are discarded until a file
/// is attached, so logging can be initialized before the run id exists.
#[derive(Clone, Default)]
pub struct RunLogSink {
    file: Arc<Mutex<Option<RedactingWriter<File>>>>,
}

impl RunLogSink {
    /// Creates `dir/run.log` (refusing to overwrite an existing one) and
    /// routes every subsequent line through `redactor` into it.
    pub fn attach(&self, dir: &Path, redactor: Redactor) -> io::Result<()> {
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join(RUN_LOG_FILE))?;
        *lock_recovering(&self.file) = Some(RedactingWriter::new(file, redactor));
        Ok(())
    }

    /// True once a file is attached.
    pub fn is_attached(&self) -> bool {
        lock_recovering(&self.file).is_some()
    }
}

/// Per-event writer handed out by [`RunLogSink`].
pub struct RunLogWriter {
    sink: RunLogSink,
    writer: Option<RedactingWriter<File>>,
}

impl Write for RunLogWriter {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        match &mut self.writer {
            Some(writer) => writer.write(buf),
            None => Ok(buf.len()),
        }
    }

    fn flush(&mut self) -> io::Result<()> {
        match &mut self.writer {
            Some(writer) => writer.flush(),
            None => Ok(()),
        }
    }
}

impl Drop for RunLogWriter {
    fn drop(&mut self) {
        // The per-writer buffer flushes in RedactingWriter::drop; keep the
        // sink alive until then.
        let _ = &self.sink;
    }
}

impl<'a> MakeWriter<'a> for RunLogSink {
    type Writer = RunLogWriter;

    fn make_writer(&'a self) -> Self::Writer {
        let writer = lock_recovering(&self.file)
            .as_ref()
            .map(RedactingWriter::handle);
        RunLogWriter {
            sink: self.clone(),
            writer,
        }
    }
}

static SINK: OnceLock<RunLogSink> = OnceLock::new();

/// Installs the global subscriber (stderr + deferred run log). Idempotent:
/// later calls return the same sink. `attach_run_log` routes the file layer
/// once a run directory exists.
pub fn init_logging() -> RunLogSink {
    let sink = SINK.get_or_init(RunLogSink::default).clone();
    // `RUST_LOG` sets both; without it, each layer has its own default.
    let filter = |default: &str| {
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new(default))
    };
    // Escape sequences only where colour is welcome: `run 2>run.err` stays
    // plain text unless the operator forces colour.
    let stderr_ansi = crate::terminal::color_on(crate::terminal::Stream::Stderr);
    let stderr_layer = tracing_subscriber::fmt::layer()
        .with_writer(io::stderr)
        .with_ansi(stderr_ansi)
        .with_target(false)
        .with_filter(filter(DEFAULT_TERMINAL_FILTER));
    let file_layer = tracing_subscriber::fmt::layer()
        .with_ansi(false)
        .with_writer(sink.clone())
        .with_filter(filter(DEFAULT_FILTER));
    // Only the first call installs; the sink is shared regardless.
    let _ = tracing_subscriber::registry()
        .with(stderr_layer)
        .with(file_layer)
        .try_init();
    sink
}

/// Attaches `dir/run.log` to the global sink with the given redactor.
pub fn attach_run_log(dir: &Path, redactor: Redactor) -> io::Result<()> {
    init_logging().attach(dir, redactor)
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::secret::Secret;

    #[derive(Default, Clone)]
    struct Sink(Arc<Mutex<Vec<u8>>>);

    impl Write for Sink {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn lines_are_redacted_before_reaching_the_sink() {
        let sink = Sink::default();
        let redactor = Redactor::new().with_secret(&Secret::new("hunter2"));
        let mut writer = RedactingWriter::new(sink.clone(), redactor);

        writer.write_all(b"login hunter2 ok\npartial hun").unwrap();
        writer.write_all(b"ter2 end\n").unwrap();
        writer.write_all(b"tail without newline").unwrap();
        writer.flush().unwrap();

        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(
            out,
            "login <redacted> ok\npartial <redacted> end\ntail without newline\n"
        );
    }

    #[test]
    fn concurrent_handles_do_not_interleave_and_drop_flushes() {
        let sink = Sink::default();
        let writer = RedactingWriter::new(sink.clone(), Redactor::new());
        let mut a = writer.handle();
        let mut b = writer.handle();

        a.write_all(b"aaa ").unwrap();
        b.write_all(b"bbb ").unwrap();
        a.write_all(b"end\n").unwrap();
        drop(b);

        let out = String::from_utf8(sink.0.lock().unwrap().clone()).unwrap();
        assert_eq!(out, "aaa end\nbbb \n");
    }

    #[test]
    fn run_log_sink_discards_until_attached_then_redacts() {
        let dir = tempfile::tempdir().unwrap();
        let sink = RunLogSink::default();
        sink.make_writer().write_all(b"lost\n").unwrap();
        assert!(!sink.is_attached());

        sink.attach(
            dir.path(),
            Redactor::new().with_secret(&Secret::new("s3cr3t")),
        )
        .unwrap();
        sink.make_writer().write_all(b"kept s3cr3t\n").unwrap();

        let log = std::fs::read_to_string(dir.path().join(RUN_LOG_FILE)).unwrap();
        assert_eq!(log, "kept <redacted>\n");
        assert!(sink.is_attached());
        assert!(
            sink.attach(dir.path(), Redactor::new()).is_err(),
            "never overwrite"
        );
    }

    #[test]
    fn init_logging_is_idempotent_and_writes_after_attach() {
        let dir = tempfile::tempdir().unwrap();
        let first = init_logging();
        let second = init_logging();
        assert!(Arc::ptr_eq(&first.file, &second.file));

        attach_run_log(
            dir.path(),
            Redactor::new().with_secret(&Secret::new("s3cr3t")),
        )
        .unwrap();
        tracing::info!(target: "mortsom", "starting with s3cr3t");

        let log = std::fs::read_to_string(dir.path().join(RUN_LOG_FILE)).unwrap();
        assert!(log.contains("<redacted>"), "{log}");
        assert!(!log.contains("s3cr3t"));
    }

    fn emit_scenario_and_progress_events(filter: &str) -> String {
        let sink = Sink::default();
        let writer = sink.clone();
        let layer = tracing_subscriber::fmt::layer()
            .with_ansi(false)
            .with_writer(move || writer.clone())
            .with_filter(EnvFilter::new(filter));
        tracing::subscriber::with_default(tracing_subscriber::registry().with(layer), || {
            tracing::info!(target: "mortsom_scenario_kit::timeline", "order shows pending");
            tracing::info!(target: "mortsom_scenario_kit::actions", "order created");
            tracing::info!(target: "mortsom_environment::lnd", "payment settled");
            tracing::info!(target: "mortsom_solver", "solver registration sent");
            tracing::info!(target: "mortsom_coordinator::run::execute", "scenario finished");
            tracing::info!(target: "mortsom_coordinator::report", "finished");
            tracing::warn!(target: "mortsom_scenario_kit::timeline", "relay quiet for 10s");
            tracing::info!(target: "mortsom_coordinator::desktop", "building v2 clients");
            tracing::info!(target: "mortsom_coordinator::stack::up", "starting mostrod");
        });
        String::from_utf8(sink.0.lock().unwrap().clone()).unwrap()
    }

    /// The terminal shows long operations and warnings; the scenario's own
    /// narrative is for `run.log` and `timeline.md`.
    #[test]
    fn the_terminal_default_keeps_progress_and_warnings_but_not_the_narrative() {
        // Arrange / Act
        let terminal = emit_scenario_and_progress_events(DEFAULT_TERMINAL_FILTER);

        // Assert
        for quiet in [
            "order shows pending",
            "order created",
            "payment settled",
            "solver registration sent",
            "scenario finished",
            " finished",
        ] {
            assert!(!terminal.contains(quiet), "{quiet:?} reached:\n{terminal}");
        }
        assert!(terminal.contains("relay quiet for 10s"), "{terminal}");
        assert!(terminal.contains("building v2 clients"), "{terminal}");
        assert!(terminal.contains("starting mostrod"), "{terminal}");
    }

    #[test]
    fn the_run_log_default_keeps_the_whole_narrative() {
        // Arrange / Act
        let log = emit_scenario_and_progress_events(DEFAULT_FILTER);

        // Assert
        assert!(log.contains("order shows pending"), "{log}");
        assert!(log.contains("scenario finished"), "{log}");
        assert!(log.contains("building v2 clients"), "{log}");
    }
}
