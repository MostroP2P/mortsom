//! The block-art banner every command opens with.
//!
//! It says three things before anything else happens: which tool this is,
//! what it does, and what this particular invocation is about to do it
//! with. The third earns its space: Mortsom drives two real clients
//! against a configured stack and moves real sats on it, so
//! `run · happy_sell · linux · config/desktop.example.toml` answers "which
//! app, on which platform, against which environment?" without reading a
//! flag back.
//!
//! Printed on **stderr**, always. `doctor --json` writes JSON on stdout
//! and `list` writes its scenario table there; a banner there would break
//! the pipe it was decorating. Its colour follows stderr for the
//! same reason: `mortsom doctor 2>doctor.log` must not fill that log with
//! escape sequences because stdout happened to be a terminal.

use std::path::Path;

use crate::cli::{Command, StackAction};
use crate::style::{self, INDENT, Style};

/// Three lines of block art, the tagline, and what this invocation is.
///
/// Blank lines above and below keep it from sitting flush against the
/// shell prompt that invoked it.
pub fn render(detail: &str, style: Style) -> String {
    const ART: [&str; 3] = [
        "▄▄ ▄▄ ▄▀▀▀▄ ▄▀▀▄ ▀▀█▀▀ ▄▀▀▀ ▄▀▀▀▄ ▄▄ ▄▄",
        "█ ▀ █ █   █ █▀▀▄   █    ▀▀▄ █   █ █ ▀ █",
        "▀   ▀ ▀▄▄▄▀ ▀  ▀   ▀   ▀▄▄▀ ▀▄▄▄▀ ▀   ▀",
    ];
    let captions = ["", "black-box e2e for Mostro client", detail];

    let mut out = String::from("\n\n");
    for (art, caption) in ART.iter().zip(captions) {
        let line = format!("{INDENT}{}", style.rgb(style::BRAND, art));
        if caption.is_empty() {
            out.push_str(&format!("{line}\n"));
        } else {
            out.push_str(&format!("{line}   {}\n", style.dim(caption)));
        }
    }
    out.push('\n');
    out
}

/// What the banner's third line says, from the parsed arguments alone.
///
/// Pure and total: no config is loaded and no registry consulted, so the
/// banner is printed before anything can fail. A command that dies on a
/// missing environment variable has still said which config it read.
pub fn caption(command: &Command, config: &Path, stack_dir: &Path) -> String {
    let config = config.display();
    match command {
        Command::Run {
            platform,
            scenarios,
            tag,
            ..
        } => {
            let selection = if !scenarios.is_empty() {
                scenarios.join(", ")
            } else if let Some(tag) = tag {
                format!("tag {tag:?}")
            } else {
                "every scenario".to_owned()
            };
            format!("run · {selection} · {platform} · {config}")
        }
        // The one command that reads no config at all.
        Command::List { .. } => "list · every scenario in the registry".to_owned(),
        Command::Doctor { platform, .. } => format!("doctor · {platform} · {config}"),
        // The run id alone would not say which artifact tree was searched:
        // it is a directory under the config's `artifacts_dir`.
        Command::Cleanup { run, .. } => format!("cleanup · run {run} · {config}"),
        Command::Recovery {
            action: crate::cli::RecoveryAction::Abandon { run, .. },
        } => format!("recovery abandon · run {run} · {}", stack_dir.display()),
        Command::Publish { run, .. } => format!("publish · run {run} · {config}"),
        Command::Parity { run_a, run_b, .. } => format!("parity · {run_a} vs {run_b} · {config}"),
        Command::Accounting { lane } => match lane {
            Some(lane) => format!("accounting · lane {lane} · {config}"),
            None => format!("accounting · every lane · {config}"),
        },
        // The stack commands read no config at all: they act on the stack
        // directory. Naming the config here would point at one environment
        // immediately before `down` destroys another.
        Command::Stack { action } => {
            let action = match action {
                StackAction::Up { .. } => "up",
                StackAction::Status => "status",
                StackAction::Logs { .. } => "logs",
                StackAction::Down => "down",
            };
            format!("stack {action} · {}", stack_dir.display())
        }
        Command::Relay { .. } => format!("relay · {config}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn config() -> PathBuf {
        PathBuf::from("config/desktop.example.toml")
    }

    fn stack_dir() -> PathBuf {
        PathBuf::from("stack")
    }

    fn a_run(scenarios: &[&str], tag: Option<&str>) -> Command {
        Command::Run {
            preserve_legacy_recovery: Vec::new(),
            platform: mortsom_desktop_driver::Platform::Linux,
            app_artifact: None,
            rebuild: false,
            source: Default::default(),
            xvfb: PathBuf::from("Xvfb"),
            scenarios: scenarios.iter().map(|s| (*s).to_owned()).collect(),
            tag: tag.map(str::to_owned),
        }
    }

    #[test]
    fn the_banner_carries_the_tool_the_tagline_and_the_invocation() {
        // Arrange / Act
        let out = render(
            "run · happy_sell · linux · config/desktop.example.toml",
            Style::plain(),
        );

        // Assert
        assert!(out.contains("▄▀▀▀▄"), "{out}");
        assert!(out.contains("black-box e2e for Mostro client"), "{out}");
        assert!(
            out.contains("run · happy_sell · linux · config/desktop.example.toml"),
            "{out}"
        );
    }

    /// The art spells MORTSOM: every row is the same width, so the letters
    /// stay in their columns whatever the caption beside them says.
    #[test]
    fn the_three_rows_of_art_are_one_rectangle() {
        // Arrange / Act
        let banner = render("list", Style::plain());
        let art: Vec<&str> = banner
            .lines()
            .filter(|line| line.contains('▀') || line.contains('▄'))
            .collect();

        // Assert
        assert_eq!(art.len(), 3, "{art:?}");
        // The captions carry no block characters, so the art ends at the
        // last one: splitting on whitespace would cut inside a letter.
        let widths: Vec<usize> = art
            .iter()
            .map(|line| {
                line.chars()
                    .enumerate()
                    .filter(|(_, c)| "▀▄█".contains(*c))
                    .map(|(i, _)| i + 1)
                    .last()
                    .unwrap_or(0)
            })
            .collect();
        assert!(
            widths.windows(2).all(|w| w[0] == w[1]),
            "rows differ in width: {widths:?}"
        );
    }

    #[test]
    fn the_banner_is_padded_away_from_the_shell_prompt() {
        // Arrange / Act
        let out = render("list", Style::plain());

        // Assert
        assert!(out.starts_with("\n\n"), "{out:?}");
        assert!(out.ends_with("\n\n"), "{out:?}");
    }

    /// Redirected output stays plain, so a captured banner reads as text.
    #[test]
    fn colour_is_emitted_only_when_asked_for() {
        // Arrange / Act
        let plain = render("list", Style::plain());
        let painted = render("list", Style::new(true, true));

        // Assert
        assert!(!plain.contains('\u{1b}'), "{plain:?}");
        assert!(
            painted.contains("\u{1b}[38;2;155;206;75m") && painted.contains("\u{1b}[2m"),
            "{painted:?}"
        );
        assert!(painted.ends_with("\n\n"), "{painted:?}");
    }

    #[test]
    fn a_run_names_its_selection_its_platform_and_its_config() {
        // Arrange / Act / Assert
        assert_eq!(
            caption(&a_run(&["happy_sell"], None), &config(), &stack_dir()),
            "run · happy_sell · linux · config/desktop.example.toml"
        );
        assert_eq!(
            caption(&a_run(&[], Some("smoke")), &config(), &stack_dir()),
            "run · tag \"smoke\" · linux · config/desktop.example.toml"
        );
        assert_eq!(
            caption(&a_run(&[], None), &config(), &stack_dir()),
            "run · every scenario · linux · config/desktop.example.toml"
        );
    }

    /// The banner's reason for existing: the config and the platform decide
    /// which environment is touched and which client is driven, and both are
    /// easy to forget on a command that takes minutes to fail.
    #[test]
    fn every_command_that_reads_a_config_names_it() {
        // Arrange
        let commands = [
            a_run(&[], None),
            Command::Doctor {
                platform: mortsom_desktop_driver::Platform::Web,
                app_artifact: None,
                source: Default::default(),
                no_network: false,
                json: false,
            },
            Command::Cleanup {
                run: "20260909T121608Z-e3265b1d41b0".to_owned(),
                attribute_order: Vec::new(),
            },
            Command::Publish {
                run: "20260909T121608Z-e3265b1d41b0".to_owned(),
                out: PathBuf::from("bundle"),
                screenshots: "all".to_owned(),
            },
            Command::Parity {
                run_a: "a".to_owned(),
                run_b: "b".to_owned(),
                intentional: None,
            },
            Command::Accounting { lane: None },
        ];

        // Act / Assert
        for command in &commands {
            let caption = caption(command, &config(), &stack_dir());
            assert!(
                caption.contains("config/desktop.example.toml"),
                "{command:?} → {caption}"
            );
        }
    }

    #[test]
    fn the_one_command_that_reads_no_config_names_what_it_reads_instead() {
        // Arrange / Act
        let list = caption(&Command::List { details: false }, &config(), &stack_dir());

        // Assert
        assert!(!list.contains("desktop.example.toml"), "{list}");
        assert!(list.contains("registry"), "{list}");
    }

    #[test]
    fn doctor_names_the_platform_whose_prerequisites_it_checks() {
        // Arrange / Act
        let web = caption(
            &Command::Doctor {
                platform: mortsom_desktop_driver::Platform::Web,
                app_artifact: None,
                source: Default::default(),
                no_network: false,
                json: false,
            },
            &config(),
            &stack_dir(),
        );

        // Assert
        assert!(web.starts_with("doctor · web · "), "{web}");
    }

    /// `stack` reads no config: it acts on the stack directory. Naming the
    /// config would point at one environment immediately before `down`
    /// destroys another.
    #[test]
    fn a_stack_action_names_the_directory_it_will_touch_not_the_config() {
        // Arrange
        let elsewhere = PathBuf::from("/srv/another-stack");
        let actions = [
            (
                StackAction::Up {
                    mostro_image: None,
                    mostro_ref: None,
                    replace: false,
                    bond: false,
                },
                "stack up",
            ),
            (StackAction::Status, "stack status"),
            (StackAction::Down, "stack down"),
        ];

        // Act / Assert
        for (action, prefix) in actions {
            let caption = caption(&Command::Stack { action }, &config(), &elsewhere);

            assert_eq!(caption, format!("{prefix} · /srv/another-stack"));
            assert!(!caption.contains("desktop.example.toml"), "{caption}");
        }
    }
}
