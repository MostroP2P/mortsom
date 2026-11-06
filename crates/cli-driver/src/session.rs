//! A `UiSession` with no screen: the coordinator's lifecycle maps onto the
//! CLI's storage, evidence becomes its transcript, and every element control
//! is unsupported. The CLI adapter reaches the client through
//! [`UiSession::command_line`].

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use mortsom_ui_driver::UiSession;
use mortsom_ui_driver::error::UiError;
use mortsom_ui_driver::process::ProcessError;

use crate::client::CliClient;
use crate::error::CliError;

/// One actor's CLI as a session.
pub struct CliSession {
    client: Arc<CliClient>,
}

impl CliSession {
    /// Wraps `client`.
    pub fn new(client: Arc<CliClient>) -> Self {
        Self { client }
    }

    /// The actor's client.
    pub fn client(&self) -> &Arc<CliClient> {
        &self.client
    }
}

const ELEMENTS: &str = "element automation";

#[async_trait]
impl UiSession for CliSession {
    async fn exists(&self, _: &str) -> Result<bool, UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn wait_for(&self, _: &str, _: Duration) -> Result<(), UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn wait_gone(&self, _: &str, _: Duration) -> Result<(), UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn tap(&self, _: &str) -> Result<(), UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn type_text(&self, _: &str, _: &str) -> Result<(), UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn text_of(&self, _: &str) -> Result<String, UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn is_enabled(&self, _: &str) -> Result<bool, UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn label_of(&self, _: &str) -> Result<String, UiError> {
        Err(UiError::Unsupported(ELEMENTS))
    }
    async fn screenshot_png(&self) -> Result<Vec<u8>, UiError> {
        Err(UiError::Unsupported("screenshots"))
    }
    /// The invocations made since the previous evidence checkpoint.
    async fn page_source(&self) -> Result<String, UiError> {
        Ok(self.client.take_transcript())
    }
    async fn background(&self, _: u64) -> Result<(), UiError> {
        Err(UiError::Unsupported("backgrounding"))
    }
    /// Every invocation is its own process: terminating waits for one a
    /// caller gave up on, up to its deadline plus a grace, and fails while
    /// it outlives that instead of reporting a stop. The store stays open.
    async fn terminate_app(&self) -> Result<(), UiError> {
        self.client.settle().await.map_err(ui_error)
    }
    async fn activate_app(&self) -> Result<(), UiError> {
        self.client.ensure_open().map_err(ui_error)
    }
    async fn clear_app(&self) -> Result<(), UiError> {
        self.client.start_fresh().map_err(ui_error)
    }
    async fn set_connectivity(&self, _: bool) -> Result<(), UiError> {
        Err(UiError::Unsupported("network control"))
    }
    async fn set_orientation(&self, _: bool) -> Result<(), UiError> {
        Err(UiError::Unsupported("rotation"))
    }
    async fn hide_keyboard(&self) -> Result<(), UiError> {
        Ok(())
    }
    async fn diagnostics(&self) -> Vec<(String, String)> {
        self.client.diagnostics().await
    }
    fn command_line(&self) -> Option<&dyn std::any::Any> {
        Some(self)
    }
    /// Ends once no invocation runs; one that outlives its deadline fails
    /// the shutdown instead of being reported as stopped.
    async fn end(&self) -> Result<(), UiError> {
        self.client.close().await.map_err(ui_error)
    }
}

fn ui_error(error: CliError) -> UiError {
    UiError::Process(ProcessError::Failed {
        command: "mostro-cli".into(),
        stderr: error.to_string(),
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used)]
mod tests {
    use super::*;
    use crate::call::{Call, NodeTarget};
    use crate::client::CliConfig;
    use crate::testing::{ScriptedCli, SlowCli};

    fn config() -> CliConfig {
        CliConfig {
            binary: "/opt/mostro-cli".into(),
            node: NodeTarget {
                mostro_pubkey: "ab".repeat(32),
                relays: vec!["ws://127.0.0.1:7500".into()],
                reply_timeout_secs: None,
            },
            timeout: Duration::from_secs(5),
            path_env: None,
        }
    }

    fn session() -> (Arc<ScriptedCli>, CliSession) {
        let cli = Arc::new(ScriptedCli::default());
        let client = Arc::new(CliClient::new(config(), cli.clone()));
        (cli, CliSession::new(client))
    }

    /// Teardown counts a session as ended only once nothing runs in it.
    #[tokio::test(start_paused = true)]
    async fn a_session_whose_invocation_outlives_its_deadline_fails_to_end() {
        let slow = Arc::new(SlowCli {
            delay: Duration::from_secs(3600),
        });
        let session = CliSession::new(Arc::new(CliClient::new(config(), slow)));
        session.clear_app().await.unwrap();
        let gave_up = tokio::time::timeout(
            Duration::from_millis(20),
            session.client().invoke(Call::local(&["identity", "show"])),
        )
        .await;
        assert!(gave_up.is_err(), "the caller should have timed out");

        let ended = session.end().await;

        assert!(ended.is_err(), "{ended:?}");
    }

    /// Gives up on one `identity show` that `session` is still running.
    async fn give_up_on_one(session: &CliSession) {
        session.clear_app().await.unwrap();
        let gave_up = tokio::time::timeout(
            Duration::from_millis(20),
            session.client().invoke(Call::local(&["identity", "show"])),
        )
        .await;
        assert!(gave_up.is_err(), "the caller should have timed out");
    }

    /// A relaunch after a step that timed out: terminating reports a stop
    /// only once the abandoned invocation has ended, and the store then
    /// opens again at once.
    #[tokio::test]
    async fn terminating_waits_for_an_invocation_whose_caller_gave_up() {
        let slow = Arc::new(SlowCli {
            delay: Duration::from_millis(150),
        });
        let session = CliSession::new(Arc::new(CliClient::new(config(), slow)));
        give_up_on_one(&session).await;

        session.terminate_app().await.unwrap();
        let recorded = session.page_source().await.unwrap();
        session.activate_app().await.unwrap();

        assert_eq!(recorded.lines().count(), 1, "{recorded}");
    }

    #[tokio::test(start_paused = true)]
    async fn terminating_fails_while_an_invocation_outlives_its_deadline() {
        let slow = Arc::new(SlowCli {
            delay: Duration::from_secs(3600),
        });
        let session = CliSession::new(Arc::new(CliClient::new(config(), slow)));
        give_up_on_one(&session).await;

        let terminated = session.terminate_app().await;

        assert!(terminated.is_err(), "{terminated:?}");
    }

    #[tokio::test]
    async fn the_screen_controls_are_unsupported_and_named() {
        let (_, session) = session();

        assert!(matches!(
            session.tap("order.status").await,
            Err(UiError::Unsupported(_))
        ));
        assert!(matches!(
            session.screenshot_png().await,
            Err(UiError::Unsupported("screenshots"))
        ));
        assert!(session.set_connectivity(false).await.is_err());
    }

    #[tokio::test]
    async fn the_page_source_is_the_transcript_of_the_step() {
        let (cli, session) = session();
        session.clear_app().await.unwrap();
        cli.ok("identity.show", serde_json::json!({"identity_pubkey": "a"}));

        session
            .client()
            .invoke(Call::local(&["identity", "show"]))
            .await
            .unwrap();
        let first = session.page_source().await.unwrap();
        let second = session.page_source().await.unwrap();

        assert!(first.contains("identity.show"), "{first}");
        assert!(second.is_empty());
        assert_eq!(session.diagnostics().await.len(), 1);
    }

    #[tokio::test]
    async fn the_adapter_finds_the_client_behind_the_session() {
        let (_, session) = session();
        let ui: &dyn UiSession = &session;

        let found = ui
            .command_line()
            .and_then(|any| any.downcast_ref::<CliSession>());

        assert!(found.is_some());
    }

    #[tokio::test]
    async fn a_session_must_be_launched_before_it_runs_anything() {
        let (_, session) = session();

        let error = session
            .client()
            .invoke(Call::local(&["identity", "show"]))
            .await
            .unwrap_err();

        assert!(matches!(error, CliError::Storage(_)), "{error}");
        session.activate_app().await.unwrap();
        session.terminate_app().await.unwrap();
        session.end().await.unwrap();
    }
}
