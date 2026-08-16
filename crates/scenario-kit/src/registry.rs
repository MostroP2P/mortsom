//! Scenario registry: scenarios self-register with `inventory`; the runner
//! never keeps a hand-maintained list.

use std::future::Future;
use std::pin::Pin;

use mortsom_app_contract::Capability;

use crate::context::{Context, ScenarioError};

/// A scenario body.
pub type ScenarioFuture<'a> = Pin<Box<dyn Future<Output = Result<(), ScenarioError>> + Send + 'a>>;
/// Function pointer that runs a scenario.
pub type ScenarioFn = for<'a> fn(&'a Context) -> ScenarioFuture<'a>;

/// A registered scenario.
pub struct ScenarioDef {
    /// Unique name (`happy_sell`).
    pub name: &'static str,
    /// Suite tags (`smoke`, `regression`, `recovery`, `adversarial`).
    pub tags: &'static [&'static str],
    /// Capabilities the profile must support.
    pub requires: &'static [Capability],
    /// Deadline for one attempt.
    pub timeout_secs: u64,
    /// True when the scenario moves sats; it then needs mainnet clearance.
    pub moves_value: bool,
    /// Body.
    pub run: ScenarioFn,
}

inventory::collect!(ScenarioDef);

/// Every registered scenario, sorted by name.
pub fn all() -> Vec<&'static ScenarioDef> {
    let mut defs: Vec<&'static ScenarioDef> = inventory::iter::<ScenarioDef>.into_iter().collect();
    defs.sort_by_key(|d| d.name);
    defs
}

/// Finds a scenario by name.
pub fn find(name: &str) -> Option<&'static ScenarioDef> {
    all().into_iter().find(|d| d.name == name)
}

/// Selects scenarios by explicit names and/or a tag. Unknown names are an
/// error, never silently ignored.
pub fn select(names: &[String], tag: Option<&str>) -> Result<Vec<&'static ScenarioDef>, String> {
    let mut selected = Vec::new();
    for name in names {
        let def = find(name).ok_or_else(|| format!("unknown scenario `{name}`"))?;
        // A name repeated on the command line runs the scenario once.
        if !selected.iter().any(|d: &&ScenarioDef| d.name == def.name) {
            selected.push(def);
        }
    }
    if let Some(tag) = tag {
        let tagged: Vec<_> = all()
            .into_iter()
            .filter(|d| d.tags.contains(&tag))
            .collect();
        if tagged.is_empty() {
            return Err(format!("no scenario carries tag `{tag}`"));
        }
        for def in tagged {
            if !selected.iter().any(|d| d.name == def.name) {
                selected.push(def);
            }
        }
    }
    if selected.is_empty() {
        return Err("no scenario selected: pass names or --tag".into());
    }
    Ok(selected)
}

/// Rejects duplicate names loudly at startup.
pub fn validate() -> Result<(), String> {
    let defs = all();
    for pair in defs.windows(2) {
        if pair[0].name == pair[1].name {
            return Err(format!("duplicate scenario name `{}`", pair[0].name));
        }
    }
    Ok(())
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;

    fn noop(_ctx: &Context) -> ScenarioFuture<'_> {
        Box::pin(async { Ok(()) })
    }

    inventory::submit! {
        ScenarioDef {
            name: "zz_kit_selftest",
            tags: &["kit-selftest"],
            requires: &[],
            timeout_secs: 1,
            moves_value: false,
            run: noop,
        }
    }

    #[test]
    fn registry_finds_and_selects() {
        assert!(find("zz_kit_selftest").is_some());
        assert!(find("nope").is_none());
        assert!(select(&["nope".into()], None).is_err());
        assert!(select(&[], Some("no-such-tag")).is_err());
        assert!(select(&[], None).is_err());
        let by_tag = select(&[], Some("kit-selftest")).unwrap_or_default();
        assert_eq!(by_tag.len(), 1);
        assert!(validate().is_ok());
    }
    #[test]
    fn a_name_repeated_on_the_command_line_runs_once() {
        let names = vec!["zz_kit_selftest".to_owned(), "zz_kit_selftest".to_owned()];

        let selected = select(&names, None).unwrap();

        assert_eq!(selected.len(), 1, "a scenario must not run twice");
    }
}
