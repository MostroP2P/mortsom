//! Verified text entry through public accessibility operations.
use crate::native::{Node, error};
use async_trait::async_trait;
use mortsom_android_driver::appium::AppiumError;
use std::time::Duration;

/// Public accessibility boundary, injectable without a desktop session.
#[async_trait]
pub trait TextEntry: Send + Sync {
    /// Fresh lookup of the semantic identifier.
    async fn find(&self, id: &str) -> Result<Option<Node>, AppiumError>;
    /// Invokes the named user tap action to request focus.
    async fn focus(&self, node: &Node) -> Result<(), AppiumError>;
    /// Reads the actual focused state.
    async fn focused(&self, node: &Node) -> Result<bool, AppiumError>;
    /// Sends text through the public editable interface.
    async fn set(&self, node: &Node, value: &str) -> Result<(), AppiumError>;
    /// Reads the public editable text value.
    async fn text(&self, node: &Node) -> Result<String, AppiumError>;
}

/// Waits for focus and verified text, never treating an acknowledgement as editing.
pub async fn enter(
    boundary: &dyn TextEntry,
    id: &str,
    value: &str,
    timeout: Duration,
) -> Result<(), AppiumError> {
    let result = tokio::time::timeout(timeout, async {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        let mut focused_node = None;
        loop {
            tick.tick().await;
            let Some(node) = boundary.find(id).await? else {
                continue;
            };
            if boundary.text(&node).await? == value {
                return Ok(());
            }
            let identity = (node.destination.clone(), node.path.clone());
            if !boundary.focused(&node).await? {
                if focused_node.as_ref() != Some(&identity) {
                    boundary.focus(&node).await?;
                    focused_node = Some(identity);
                }
                continue;
            }
            // Text replacement is idempotent. A focused Flutter node may
            // acknowledge before its editing handler is ready; retry only
            // while the public value still differs and the deadline remains.
            boundary.set(&node, value).await?;
        }
    })
    .await
    .map_err(|_| error("text-entry deadline: public value mismatch"))?;
    let redactor = mortsom_common::Redactor::new().with_secret(&mortsom_common::Secret::new(value));
    result.map_err(|cause: AppiumError| error(redactor.redact(&cause.to_string())))
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    #[derive(Default)]
    struct State {
        finds: u32,
        focused_reads: u32,
        writes: u32,
        requested_focus: bool,
        text: String,
    }
    struct Fake {
        state: Mutex<State>,
        ignore_write: bool,
        ignore_first: bool,
        error_text: Option<&'static str>,
    }
    #[async_trait]
    impl TextEntry for Fake {
        async fn find(&self, _: &str) -> Result<Option<Node>, AppiumError> {
            let mut state = self.state.lock().unwrap();
            state.finds += 1;
            Ok(Some(Node {
                destination: ":1.1".into(),
                path: if state.finds > 1 { "/new" } else { "/old" }.into(),
                name: "[mortsom:input]".into(),
            }))
        }
        async fn focus(&self, _: &Node) -> Result<(), AppiumError> {
            self.state.lock().unwrap().requested_focus = true;
            Ok(())
        }
        async fn focused(&self, _: &Node) -> Result<bool, AppiumError> {
            let mut state = self.state.lock().unwrap();
            state.focused_reads += 1;
            Ok(state.requested_focus && state.focused_reads > 2)
        }
        async fn set(&self, node: &Node, value: &str) -> Result<(), AppiumError> {
            let mut state = self.state.lock().unwrap();
            assert!(state.requested_focus && state.focused_reads > 2);
            assert_eq!(node.path, "/new");
            state.writes += 1;
            if !self.ignore_write && !(self.ignore_first && state.writes == 1) {
                state.text = value.into();
            }
            Ok(())
        }
        async fn text(&self, _: &Node) -> Result<String, AppiumError> {
            if let Some(text) = self.error_text {
                return Err(error(format!("backend rejected: {text}")));
            }
            Ok(self.state.lock().unwrap().text.clone())
        }
    }
    #[tokio::test]
    async fn waits_for_focus_and_refinds_rebuilt_nodes_before_editing() {
        let fake = Fake {
            state: Mutex::default(),
            ignore_write: false,
            ignore_first: false,
            error_text: None,
        };
        assert!(
            enter(&fake, "input", "123", Duration::from_secs(1))
                .await
                .is_ok()
        );
        assert_eq!(fake.state.lock().unwrap().writes, 1);
    }
    #[tokio::test]
    async fn accepted_but_unchanged_text_fails_without_exposing_the_value() {
        let fake = Fake {
            state: Mutex::default(),
            ignore_write: true,
            ignore_first: false,
            error_text: None,
        };
        let error = enter(
            &fake,
            "input",
            "sensitive-entry",
            Duration::from_millis(250),
        )
        .await
        .unwrap_err();
        assert!(!error.to_string().contains("sensitive-entry"));
        assert!(fake.state.lock().unwrap().writes >= 1);
    }
    #[tokio::test]
    async fn boundary_failures_cannot_echo_the_submitted_secret() {
        let fake = Fake {
            state: Mutex::default(),
            ignore_write: false,
            ignore_first: false,
            error_text: Some("private-entry"),
        };
        let failure = enter(&fake, "input", "private-entry", Duration::from_secs(1))
            .await
            .unwrap_err();
        assert!(!failure.to_string().contains("private-entry"));
    }
    #[tokio::test]
    async fn retries_an_ignored_idempotent_edit_while_the_public_value_is_unchanged() {
        let fake = Fake {
            state: Mutex::default(),
            ignore_write: false,
            ignore_first: true,
            error_text: None,
        };
        assert!(
            enter(&fake, "input", "123", Duration::from_secs(1))
                .await
                .is_ok()
        );
        assert_eq!(fake.state.lock().unwrap().writes, 2);
    }
}
