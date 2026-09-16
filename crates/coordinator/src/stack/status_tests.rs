#![allow(clippy::unwrap_used)]
//! How `stack status` reads: the badges, the rules and the verdict. Pure,
//! so the shape of the answer is asserted without a stack running.
use std::path::Path;

use super::super::connections::Connections;
use super::super::descriptor::Descriptor;
use super::super::metadata::{DaemonImage, DeclaredLimits};
use super::super::nodes::Service;
use super::{Node, Status};

const IMAGE_ID: &str = "sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

fn descriptor() -> Descriptor {
    Descriptor::new(
        "8f3a".to_owned(),
        Connections::for_stack(Path::new("/stack")),
        DaemonImage {
            image_id: IMAGE_ID.to_owned(),
            reference: "mostrop2p/mostro:v0.18.7".to_owned(),
            repository_digest: None,
        },
        DeclaredLimits {
            order_expiration_secs: 900,
            hold_invoice_expiry_secs: 900,
            buyer_invoice_deadline_secs: 900,
            fiat_window_secs: None,
            payout_invoice_min_expiry_secs: 900,
            max_inflight_payouts_per_destination: None,
            payout_dispatch_permits: None,
        },
    )
    .unwrap()
}

fn service(name: &str, state: &str) -> Service {
    Service {
        name: name.to_owned(),
        state: state.to_owned(),
    }
}

fn node(name: &str, channels: Option<usize>, sats: Option<u64>) -> Node {
    Node {
        name: name.to_owned(),
        active_channels: channels,
        confirmed_sats: sats,
    }
}

/// Everything up, every node funded and the recorded image running.
fn ready() -> Status {
    Status {
        services: [
            "bitcoind",
            "lnd-alice",
            "lnd-bob",
            "lnd-carol",
            "relay",
            "mostro",
        ]
        .into_iter()
        .map(|name| service(name, "running"))
        .collect(),
        nodes: ["alice", "bob", "carol"]
            .into_iter()
            .map(|name| node(name, Some(1), Some(99_000)))
            .collect(),
        descriptor: Some(descriptor()),
        daemon_matches_image: true,
    }
}

fn strip_ansi(text: &str) -> String {
    let mut out = String::new();
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{1b}' {
            out.push(c);
            continue;
        }
        for c in chars.by_ref() {
            if c == 'm' {
                break;
            }
        }
    }
    out
}

/// Evidence and anything reading this report must never carry escape
/// sequences, so plain stays the default.
#[test]
fn the_plain_rendering_has_no_escape_sequences() {
    // Arrange
    let status = ready();

    // Act
    let plain = status.render();

    // Assert
    assert!(!plain.contains('\u{1b}'), "{plain:?}");
}

/// Colour only ever wraps text that is already there, so a piped status
/// and a terminal one say the same words.
#[test]
fn colour_leaves_the_text_itself_unchanged() {
    // Arrange
    let status = ready();

    // Act
    let coloured = status.render_with_color(true);

    // Assert
    assert!(coloured.contains('\u{1b}'));
    assert_eq!(strip_ansi(&coloured), status.render());
}

/// The badge is what the eye finds in a screenful of output: a running
/// service must not read like a stopped one.
#[test]
fn a_running_service_is_badged_apart_from_a_stopped_one() {
    // Arrange
    let mut status = ready();
    status.services[5] = service("mostro", "exited");

    // Act
    let rendered = status.render();

    // Assert
    assert!(rendered.contains("[UP] bitcoind (running)"), "{rendered}");
    assert!(rendered.contains("[DOWN] mostro (exited)"), "{rendered}");
}

/// A container that was removed rather than stopped is absent from `ps`
/// entirely, which must not read as "everything listed is running".
#[test]
fn a_service_with_no_container_reads_as_gone() {
    // Arrange
    let mut status = ready();
    status.services.retain(|service| service.name != "relay");

    // Act
    let rendered = status.render();

    // Assert
    assert!(
        rendered.contains("[GONE] relay (no container)"),
        "{rendered}"
    );
}

/// A node answering about channels but holding none is up and idle;
/// rounding that to "unreachable" hides which half is wrong.
#[test]
fn an_unreachable_node_is_told_apart_from_an_idle_one() {
    // Arrange
    let mut status = ready();
    status.nodes[0] = node("alice", None, None);
    status.nodes[1] = node("bob", Some(0), Some(0));

    // Act
    let rendered = status.render();

    // Assert
    assert!(
        rendered.contains("[--] lnd-alice: unreachable"),
        "{rendered}"
    );
    assert!(
        rendered.contains("[--] lnd-bob: 0 active channel(s), 0 confirmed sats"),
        "{rendered}"
    );
    assert!(
        rendered.contains("[OK] lnd-carol: 1 active channel(s), 99000 confirmed sats"),
        "{rendered}"
    );
}

/// The two facts on a node line come from two separate `lncli` calls, so
/// one can answer while the other fails. A green badge beside a detail
/// saying a probe never came back tells the operator two different things,
/// and the failed probe is the one they would miss.
#[test]
fn a_node_with_one_failed_probe_is_not_badged_ok() {
    // Arrange: alice answers about channels but not balance; bob the
    // other way round.
    let mut status = ready();
    status.nodes[0] = node("alice", Some(1), None);
    status.nodes[1] = node("bob", None, Some(99_000));

    // Act
    let rendered = status.render();

    // Assert
    assert!(
        rendered.contains("[--] lnd-alice: 1 active channel(s), balance did not answer"),
        "{rendered}"
    );
    assert!(
        rendered.contains("[--] lnd-bob: channels did not answer, 99000 confirmed sats"),
        "{rendered}"
    );
    assert!(!rendered.contains("[OK] lnd-alice"), "{rendered}");
    assert!(!rendered.contains("[OK] lnd-bob"), "{rendered}");
}

/// The rule carries no newline of its own, so a report that forgets one
/// prints the heading welded to the first service.
#[test]
fn the_heading_sits_on_its_own_line_between_two_rules() {
    // Arrange
    let status = ready();

    // Act
    let rendered = status.render();

    // Assert
    let lines: Vec<&str> = rendered.lines().collect();
    assert!(lines[0].trim().starts_with('─'), "{rendered}");
    assert_eq!(lines[1].trim(), "stack", "{rendered}");
    assert!(lines[2].trim().starts_with('─'), "{rendered}");
    assert!(lines[3].contains("bitcoind"), "{rendered}");
}

/// The widest line this report prints is a daemon image pinned by digest,
/// which is far past any fixed width worth hard-coding. The frame is drawn
/// to the content so that line never overhangs it.
#[test]
fn the_frame_spans_the_widest_line_the_report_prints() {
    // Arrange
    let status = ready();

    // Act
    let rendered = status.render();

    // Assert: measured over the content only, since the rules are what is
    // being sized and would otherwise answer the question themselves.
    let lines: Vec<&str> = rendered.lines().collect();
    let rule = lines[0].chars().count();
    let widest = lines
        .iter()
        .filter(|line| !line.contains('─'))
        .map(|line| line.chars().count())
        .max()
        .unwrap();
    assert_eq!(
        rule, widest,
        "the frame does not fit the content: {rendered}"
    );
}

/// The verdict is the line a tired operator looks at first, and it must
/// still name what is not running.
#[test]
fn the_verdict_names_what_is_not_running() {
    // Arrange
    let whole = ready();
    let mut broken = ready();
    broken.services.retain(|service| service.name != "mostro");

    // Act / Assert
    assert!(whole.render().contains("ready"), "{}", whole.render());
    assert!(
        broken.render().contains("not ready; not running: mostro"),
        "{}",
        broken.render()
    );
}

/// Which build is running decides what a result means, so the recorded
/// identity and image stay on the report.
#[test]
fn the_identity_and_image_in_effect_are_named() {
    // Arrange
    let status = ready();

    // Act
    let rendered = status.render();

    // Assert
    assert!(rendered.contains("daemon identity: 8f3a"), "{rendered}");
    assert!(
        rendered.contains("daemon image: mostrop2p/mostro:v0.18.7"),
        "{rendered}"
    );
}

/// A daemon container that is not the recorded image makes every result
/// meaningless, so it is flagged rather than mentioned in passing.
#[test]
fn a_daemon_image_mismatch_is_flagged() {
    // Arrange
    let mut status = ready();
    status.daemon_matches_image = false;

    // Act
    let rendered = status.render();

    // Assert
    assert!(
        rendered.contains("the running container is not this image"),
        "{rendered}"
    );
    assert!(!status.is_ready());
}

/// Before `up` completes there is no identity at all, and the report has
/// to say what to run.
#[test]
fn a_stack_without_an_identity_says_what_to_run() {
    // Arrange
    let mut status = ready();
    status.descriptor = None;

    // Act
    let rendered = status.render();

    // Assert
    assert!(rendered.contains("`mortsom stack up`"), "{rendered}");
    assert!(!status.is_ready());
}
