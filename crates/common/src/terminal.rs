//! Whether a stream welcomes colour, and how many colours it can show.
//!
//! The decision is made per stream, never per process: the report goes to
//! stdout and the banner and logs to stderr, and redirecting one must not
//! decide the other. An explicit opt-out (`NO_COLOR`) wins; then an explicit
//! opt-in for pipes (`CLICOLOR_FORCE`, `FORCE_COLOR`); then whether the
//! stream is a terminal that is not `dumb`. The depth comes from `COLORTERM`:
//! 24-bit where the terminal advertises it, the 256-colour cube elsewhere.

use std::io::IsTerminal;

/// Which stream a piece of output is headed for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Stream {
    /// Reports and listings.
    Stdout,
    /// The banner, progress and logs.
    Stderr,
}

/// The colour-related environment, read once.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ColorEnv {
    /// `NO_COLOR` is set to a non-empty value.
    pub no_color: bool,
    /// `CLICOLOR_FORCE` or `FORCE_COLOR` asks for colour even on a pipe.
    pub forced: bool,
    /// `TERM` is unset or `dumb`.
    pub dumb: bool,
    /// `COLORTERM` advertises 24-bit colour.
    pub truecolor: bool,
}

impl ColorEnv {
    /// Reads the environment through `var`, so tests inject their own.
    pub fn read(var: impl Fn(&str) -> Option<String>) -> Self {
        // `0` is how both conventions spell "do not force".
        let forces = |name: &str| var(name).is_some_and(|value| value != "0");
        Self {
            no_color: var("NO_COLOR").is_some_and(|value| !value.is_empty()),
            forced: forces("CLICOLOR_FORCE") || forces("FORCE_COLOR"),
            dumb: var("TERM").is_none_or(|term| term == "dumb"),
            truecolor: matches!(var("COLORTERM").as_deref(), Some("truecolor" | "24bit")),
        }
    }

    /// The process environment.
    pub fn from_process() -> Self {
        Self::read(|name| std::env::var(name).ok())
    }

    /// Whether output headed for a stream that `is_terminal` (or not)
    /// should carry colour.
    pub fn color(&self, is_terminal: bool) -> bool {
        if self.no_color {
            return false;
        }
        self.forced || (is_terminal && !self.dumb)
    }
}

/// Whether `stream` is attached to a terminal.
pub fn is_terminal(stream: Stream) -> bool {
    match stream {
        Stream::Stdout => std::io::stdout().is_terminal(),
        Stream::Stderr => std::io::stderr().is_terminal(),
    }
}

/// Whether output headed for `stream` should carry colour, in this process.
pub fn color_on(stream: Stream) -> bool {
    ColorEnv::from_process().color(is_terminal(stream))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn env(vars: &[(&str, &str)]) -> ColorEnv {
        ColorEnv::read(|name| {
            vars.iter()
                .find(|(key, _)| *key == name)
                .map(|(_, value)| (*value).to_owned())
        })
    }

    #[test]
    fn a_terminal_gets_colour_and_a_pipe_does_not() {
        // Arrange
        let env = env(&[("TERM", "xterm-256color")]);

        // Act / Assert
        assert!(env.color(true));
        assert!(!env.color(false));
    }

    #[test]
    fn no_color_beats_everything() {
        // Arrange
        let env = env(&[
            ("TERM", "xterm-256color"),
            ("NO_COLOR", "1"),
            ("CLICOLOR_FORCE", "1"),
        ]);

        // Act / Assert
        assert!(!env.color(true));
        assert!(!env.color(false));
    }

    #[test]
    fn an_empty_no_color_is_not_an_opt_out() {
        // Arrange
        let env = env(&[("TERM", "xterm"), ("NO_COLOR", "")]);

        // Act / Assert
        assert!(env.color(true));
    }

    #[test]
    fn clicolor_force_or_force_color_colour_a_pipe() {
        for forcing in ["CLICOLOR_FORCE", "FORCE_COLOR"] {
            // Arrange
            let env = env(&[(forcing, "1")]);

            // Act / Assert: even without TERM, the operator asked.
            assert!(env.color(false), "{forcing}");
        }
    }

    #[test]
    fn a_zero_does_not_force_colour() {
        for forcing in ["CLICOLOR_FORCE", "FORCE_COLOR"] {
            // Arrange
            let env = env(&[(forcing, "0")]);

            // Act / Assert
            assert!(!env.color(false), "{forcing}");
        }
    }

    #[test]
    fn a_dumb_or_unnamed_terminal_gets_no_colour_unless_forced() {
        // Arrange
        let dumb = env(&[("TERM", "dumb")]);
        let unnamed = env(&[]);
        let forced = env(&[("TERM", "dumb"), ("FORCE_COLOR", "1")]);

        // Act / Assert
        assert!(!dumb.color(true));
        assert!(!unnamed.color(true));
        assert!(forced.color(true));
    }

    #[test]
    fn truecolor_is_what_colorterm_advertises() {
        // Arrange / Act / Assert
        assert!(env(&[("COLORTERM", "truecolor")]).truecolor);
        assert!(env(&[("COLORTERM", "24bit")]).truecolor);
        assert!(!env(&[("COLORTERM", "yes")]).truecolor);
        assert!(!env(&[]).truecolor);
    }
}
