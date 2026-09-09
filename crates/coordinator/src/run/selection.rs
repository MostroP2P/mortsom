//! Desktop invocations cannot reuse actor storage across scenarios yet.
use mortsom_scenario_kit::registry::{ScenarioDef, select};

/// Resolves aliases/tags before enforcing fresh-actor execution boundaries.
pub fn select_for_run(
    names: &[String],
    tag: Option<&str>,
    desktop: bool,
) -> Result<Vec<&'static ScenarioDef>, String> {
    let selected = select(names, tag)?;
    if desktop && selected.len() != 1 {
        return Err(
            "desktop runs require exactly one scenario; use separate commands for fresh Alice and Bob"
                .into(),
        );
    }
    Ok(selected)
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    #[test]
    fn repeated_names_are_one_scenario_and_android_keeps_batch_support() {
        mortsom_scenarios::link();
        assert_eq!(
            select_for_run(&["happy_sell".into(), "happy_sell".into()], None, true)
                .unwrap()
                .len(),
            1
        );
        let names = ["happy_sell".into(), "cancel_before_taken".into()];
        assert!(select_for_run(&names, None, true).is_err());
        assert_eq!(select_for_run(&names, None, false).unwrap().len(), 2);
    }
}
