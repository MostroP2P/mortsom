//! Typed retries for vanished public accessibility objects, never message text.
use atspi::zbus;
use mortsom_ui_driver::error::UiError;
use std::{future::Future, time::Duration};

pub(crate) enum AccessError {
    Stale,
    /// The control is there but cannot take the request yet, such as a
    /// button the app has not enabled: it is found again and retried until
    /// the deadline, which then reports this cause.
    NotYet(UiError),
    Permanent(UiError),
}
impl From<UiError> for AccessError {
    fn from(cause: UiError) -> Self {
        Self::Permanent(cause)
    }
}

pub(crate) fn classify(cause: zbus::Error) -> AccessError {
    let stale = match &cause {
        zbus::Error::FDO(cause) => matches!(
            cause.as_ref(),
            zbus::fdo::Error::UnknownObject(_) | zbus::fdo::Error::UnknownInterface(_)
        ),
        zbus::Error::MethodError(name, _, _) => matches!(
            name.as_str(),
            "org.freedesktop.DBus.Error.UnknownObject"
                | "org.freedesktop.DBus.Error.UnknownInterface"
        ),
        _ => false,
    };
    if stale {
        AccessError::Stale
    } else {
        AccessError::Permanent(crate::native::error(cause))
    }
}

pub(crate) async fn retry<T, F, Fut>(timeout: Duration, mut operation: F) -> Result<T, UiError>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, AccessError>>,
{
    tokio::time::timeout(timeout, async {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tick.tick().await;
            match operation().await {
                Ok(value) => return Ok(value),
                Err(AccessError::Stale | AccessError::NotYet(_)) => continue,
                Err(AccessError::Permanent(cause)) => return Err(cause),
            }
        }
    })
    .await
    .map_err(|_| crate::native::error("accessibility tree remained stale until deadline"))?
}

pub(crate) async fn with_fresh_node<T, Find, Found, Read, ReadResult>(
    node: &crate::native::Node,
    timeout: Duration,
    mut find: Find,
    mut read: Read,
) -> Result<T, UiError>
where
    Find: FnMut(String) -> Found,
    Found: Future<Output = Result<Option<crate::native::Node>, UiError>>,
    Read: FnMut(crate::native::Node) -> ReadResult,
    ReadResult: Future<Output = Result<T, AccessError>>,
{
    let (id, _) = crate::native::label(&node.name)
        .ok_or_else(|| crate::native::error("control has no contract identifier"))?;
    let mut current = Some(node.clone());
    let mut not_yet = None;
    let outcome = tokio::time::timeout(timeout, async {
        let mut tick = tokio::time::interval(Duration::from_millis(50));
        loop {
            tick.tick().await;
            let node = match current.take() {
                Some(node) => Some(node),
                None => find(id.to_owned()).await?,
            };
            let Some(node) = node else {
                continue;
            };
            match read(node).await {
                Ok(value) => return Ok(value),
                Err(AccessError::Stale) => continue,
                Err(AccessError::NotYet(cause)) => not_yet = Some(cause),
                Err(AccessError::Permanent(cause)) => return Err(cause),
            }
        }
    })
    .await;
    outcome.unwrap_or_else(|_| {
        Err(not_yet.unwrap_or_else(|| {
            crate::native::error("accessibility control remained stale until deadline")
        }))
    })
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    #[tokio::test]
    async fn stale_read_refinds_the_semantic_identifier_before_retrying() {
        let finds = AtomicUsize::new(0);
        let reads = AtomicUsize::new(0);
        let old = crate::native::Node {
            destination: ":1.1".into(),
            path: "/old".into(),
            name: "[mortsom:input]".into(),
        };
        let fresh = crate::native::Node {
            path: "/new".into(),
            ..old.clone()
        };
        let result = with_fresh_node(
            &old,
            Duration::from_secs(1),
            |id| {
                assert_eq!(id, "input");
                finds.fetch_add(1, Ordering::SeqCst);
                let node = fresh.clone();
                async move { Ok(Some(node)) }
            },
            |node| {
                reads.fetch_add(1, Ordering::SeqCst);
                async move {
                    if node.path == "/old" {
                        Err(AccessError::Stale)
                    } else {
                        Ok("current value")
                    }
                }
            },
        )
        .await
        .unwrap();
        assert_eq!(result, "current value");
        assert_eq!(finds.load(Ordering::SeqCst), 1);
        assert_eq!(reads.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn an_uncertain_action_failure_never_repeats_the_action() {
        let calls = AtomicUsize::new(0);
        let node = crate::native::Node {
            destination: ":1.1".into(),
            path: "/submit".into(),
            name: "[mortsom:submit]".into(),
        };
        assert!(
            with_fresh_node::<(), _, _, _, _>(
                &node,
                Duration::from_secs(1),
                |_| async { panic!("must not refind after an uncertain invocation") },
                |_| {
                    calls.fetch_add(1, Ordering::SeqCst);
                    async {
                        Err(AccessError::Permanent(crate::native::error(
                            "action response lost",
                        )))
                    }
                }
            )
            .await
            .is_err()
        );
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn retries_typed_stale_objects_but_never_a_transport_failure_named_stale() {
        let calls = AtomicUsize::new(0);
        let result = retry(Duration::from_secs(1), || async {
            match calls.fetch_add(1, Ordering::SeqCst) {
                0 => Err(classify(
                    zbus::fdo::Error::UnknownObject("gone".into()).into(),
                )),
                _ => Ok("fresh tree"),
            }
        })
        .await
        .unwrap();
        assert_eq!(result, "fresh tree");
        assert_eq!(calls.load(Ordering::SeqCst), 2);
        let calls = AtomicUsize::new(0);
        let failure = retry::<(), _, _>(Duration::from_secs(1), || async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(classify(
                zbus::fdo::Error::Failed("UnknownObject stale".into()).into(),
            ))
        })
        .await;
        assert!(failure.is_err());
        assert_eq!(calls.load(Ordering::SeqCst), 1);
    }

    /// A control that never takes the request fails at the deadline with
    /// its own cause, not as a stale tree.
    #[tokio::test]
    async fn a_control_that_never_takes_actions_fails_with_why_at_the_deadline() {
        let node = crate::native::Node {
            destination: ":1.1".into(),
            path: "/submit".into(),
            name: "[mortsom:submit]".into(),
        };
        let failure = with_fresh_node::<(), _, _, _, _>(
            &node,
            Duration::from_millis(200),
            |_| {
                let node = node.clone();
                async move { Ok(Some(node)) }
            },
            |_| async {
                Err(AccessError::NotYet(crate::native::error(
                    "control exposes no default user action",
                )))
            },
        )
        .await
        .unwrap_err();
        assert!(
            failure.to_string().contains("no default user action"),
            "{failure}"
        );
    }

    #[tokio::test]
    async fn a_permanently_stale_tree_reaches_its_deadline() {
        assert!(
            retry::<(), _, _>(Duration::from_millis(75), || async {
                Err(AccessError::Stale)
            })
            .await
            .is_err()
        );
    }

    #[test]
    fn classifies_named_dbus_errors_without_considering_their_description() {
        let message = zbus::Message::method_call("/test", "Read")
            .unwrap()
            .build(&())
            .unwrap();
        let stale = zbus::Error::MethodError(
            "org.freedesktop.DBus.Error.UnknownObject"
                .try_into()
                .unwrap(),
            None,
            message.clone(),
        );
        assert!(matches!(classify(stale), AccessError::Stale));
        let unknown = zbus::Error::MethodError(
            "org.freedesktop.DBus.Error.NoReply".try_into().unwrap(),
            Some("UnknownObject".into()),
            message,
        );
        assert!(matches!(classify(unknown), AccessError::Permanent(_)));
    }
}
