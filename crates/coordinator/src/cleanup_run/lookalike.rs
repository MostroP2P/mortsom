//! Catch a mistyped run id before cleanup inspects the host for it.
//!
//! Run ids are long timestamp-plus-nonce strings that operators copy by hand
//! from a guard message, so a dropped or doubled character is the common
//! failure. A cleanup for an id nothing on this host has ever used would
//! still probe containers and emulators and then print an ambiguous
//! "run directory: not found" report; naming the id the operator almost
//! certainly meant is more useful than that.

use std::path::Path;

/// Furthest an id may be from a known one and still count as a typo.
const MAX_TYPO_DISTANCE: usize = 3;

/// Known run ids that `requested` looks like a typo of, closest first.
///
/// Empty when `requested` is itself known, or when nothing is close enough
/// to be a plausible typo; the caller then treats the id as an unknown but
/// possible run and proceeds.
pub fn suggest_known_runs<'a>(
    requested: &str,
    known: impl IntoIterator<Item = &'a str>,
) -> Vec<String> {
    let mut candidates: Vec<(usize, String)> = known
        .into_iter()
        .filter(|id| *id != requested)
        .map(|id| (levenshtein(requested, id), id.to_owned()))
        .filter(|(distance, _)| *distance <= MAX_TYPO_DISTANCE)
        .collect();
    candidates.sort();
    candidates.into_iter().map(|(_, id)| id).collect()
}

/// Directory names directly under `dir`; an unreadable directory contributes nothing.
pub fn run_ids_in(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    entries
        .filter_map(Result::ok)
        .filter(|entry| entry.path().is_dir())
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect()
}

/// Edit distance between two strings.
fn levenshtein(a: &str, b: &str) -> usize {
    let b: Vec<char> = b.chars().collect();
    let mut previous: Vec<usize> = (0..=b.len()).collect();
    for (i, ca) in a.chars().enumerate() {
        let mut current = vec![i + 1];
        for (j, cb) in b.iter().enumerate() {
            let substitution = previous[j] + usize::from(ca != *cb);
            let insertion = current[j] + 1;
            let deletion = previous[j + 1] + 1;
            current.push(substitution.min(insertion).min(deletion));
        }
        previous = current;
    }
    previous[b.len()]
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    const KNOWN: [&str; 3] = [
        "20260909T183917Z-e8bb95fa3951",
        "20260909T180201Z-8743a4025779",
        "20260908T160318Z-e617fd03c704",
    ];

    #[test]
    fn a_dropped_trailing_character_names_the_run_it_came_from() {
        // Arrange
        let requested = "20260909T183917Z-e8bb95fa395";

        // Act
        let suggestions = suggest_known_runs(requested, KNOWN);

        // Assert
        assert_eq!(
            suggestions,
            vec!["20260909T183917Z-e8bb95fa3951".to_owned()]
        );
    }

    #[test]
    fn an_exactly_known_id_needs_no_suggestion() {
        assert!(suggest_known_runs(KNOWN[0], KNOWN).is_empty());
    }

    #[test]
    fn an_unrelated_id_is_not_guessed_at() {
        assert!(suggest_known_runs("never-ran", KNOWN).is_empty());
    }

    #[test]
    fn closer_matches_come_first() {
        let suggestions = suggest_known_runs("run-aa", ["run-b", "run-a", "other"]);
        assert_eq!(suggestions, vec!["run-a".to_owned(), "run-b".to_owned()]);
    }

    #[test]
    fn edit_distance_counts_insertions_deletions_and_substitutions() {
        assert_eq!(levenshtein("", "abc"), 3);
        assert_eq!(levenshtein("kitten", "sitting"), 3);
        assert_eq!(levenshtein("same", "same"), 0);
    }

    #[test]
    fn only_directories_count_as_runs() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(dir.path().join("run-a")).unwrap();
        std::fs::write(dir.path().join("stray.json"), "{}").unwrap();
        assert_eq!(run_ids_in(dir.path()), vec!["run-a".to_owned()]);
        assert!(run_ids_in(&dir.path().join("missing")).is_empty());
    }
}
