//! Linux accessibility traversal and actions over an actor-owned AT-SPI bus.
use crate::native_retry::{AccessError, classify, retry, with_fresh_node};
use atspi::proxy::{
    accessible::AccessibleProxy, action::ActionProxy, editable_text::EditableTextProxy,
    text::TextProxy,
};
use atspi::zbus::{self, Connection};
use mortsom_android_driver::appium::AppiumError;
use mortsom_android_driver::webdriver::WebDriverError;
use std::time::Duration;

/// One public accessibility object.
#[derive(Debug, Clone)]
pub struct Node {
    /// D-Bus unique application name.
    pub destination: String,
    /// Public accessibility object path.
    pub path: String,
    /// Accessible name, including the app's Linux identifier prefix.
    pub name: String,
}

/// Converts transport errors without exposing application data.
pub fn error(cause: impl std::fmt::Display) -> AppiumError {
    AppiumError::WebDriver(WebDriverError::Transport(
        mortsom_common::Redactor::default().redact(&format!("Linux accessibility: {cause}")),
    ))
}

/// Extracts a unique contract line, allowing Flutter's merged tab-position label.
pub fn label(name: &str) -> Option<(&str, &str)> {
    let mut markers = name
        .match_indices("[mortsom:")
        .filter(|(index, _)| *index == 0 || name.as_bytes()[index - 1] == b'\n');
    let (index, _) = markers.next()?;
    if markers.next().is_some() {
        return None;
    }
    let (id, value) = name[index..].strip_prefix("[mortsom:")?.split_once(']')?;
    if id.is_empty() || id.contains(['\n', '\r']) {
        return None;
    }
    Some((id, value))
}

/// Public AT-SPI connection. Its bus belongs only to one actor.
pub struct Native {
    connection: Connection,
}

impl Native {
    /// Connects to the actor's private accessibility bus.
    pub async fn connect(address: &str) -> Result<Self, AppiumError> {
        let connection = zbus::connection::Builder::address(address)
            .map_err(error)?
            .build()
            .await
            .map_err(error)?;
        let proxy = zbus::fdo::DBusProxy::new(&connection)
            .await
            .map_err(error)?;
        tokio::time::timeout(std::time::Duration::from_secs(15), async {
            let mut tick = tokio::time::interval(std::time::Duration::from_millis(100));
            loop {
                tick.tick().await;
                if proxy
                    .name_has_owner("org.a11y.atspi.Registry".try_into().map_err(error)?)
                    .await
                    .map_err(error)?
                {
                    return Ok::<_, AppiumError>(());
                }
            }
        })
        .await
        .map_err(error)??;
        drop(proxy);
        Ok(Self { connection })
    }

    async fn proxy<'a>(&'a self, node: &'a Node) -> Result<AccessibleProxy<'a>, AccessError> {
        AccessibleProxy::builder(&self.connection)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(classify)
    }

    /// Reads the current public tree, bounded by node count and time at caller.
    pub async fn nodes(&self) -> Result<Vec<Node>, AppiumError> {
        retry(Duration::from_secs(10), || self.nodes_once()).await
    }

    async fn nodes_once(&self) -> Result<Vec<Node>, AccessError> {
        let mut pending = vec![Node {
            destination: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
            name: String::new(),
        }];
        let mut nodes = Vec::new();
        while let Some(mut node) = pending.pop() {
            if nodes.len() >= 5000 {
                return Err(error("accessibility tree exceeds limit").into());
            }
            let proxy = self.proxy(&node).await?;
            let name = proxy.name().await.map_err(classify)?;
            let children = proxy.get_children().await.map_err(classify)?;
            drop(proxy);
            node.name = name;
            for child in children {
                if let Some(destination) = child.name_as_str() {
                    pending.push(Node {
                        destination: destination.into(),
                        path: child.path_as_str().into(),
                        name: String::new(),
                    });
                }
            }
            nodes.push(node);
        }
        Ok(nodes)
    }

    /// Finds an exact namespaced identifier, never localized copy.
    pub async fn find(&self, id: &str) -> Result<Option<Node>, AppiumError> {
        let nodes = tokio::time::timeout(std::time::Duration::from_secs(10), self.nodes())
            .await
            .map_err(error)??;
        Ok(nodes
            .into_iter()
            .find(|node| label(&node.name).is_some_and(|(key, _)| key == id)))
    }

    /// Invokes the accessible object's default user action.
    pub async fn click(&self, node: &Node) -> Result<(), AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.click_once(&node).await },
        )
        .await
    }

    async fn click_once(&self, node: &Node) -> Result<(), AccessError> {
        let proxy = ActionProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(classify)?;
        // A read-only check may detect a rebuilt node before invocation. Once
        // DoAction starts, every error is permanent to this attempt: replaying
        // a business action after a lost acknowledgement could submit twice.
        if proxy.n_actions().await.map_err(classify)? <= 0 {
            return Err(error("control exposes no default user action").into());
        }
        if proxy.do_action(0).await.map_err(error)? {
            Ok(())
        } else {
            Err(error("action refused").into())
        }
    }

    /// Focuses and verifies text through public accessibility state.
    pub async fn type_text(&self, node: &Node, value: &str) -> Result<(), AppiumError> {
        let (id, _) =
            label(&node.name).ok_or_else(|| error("text control has no contract identifier"))?;
        crate::text_entry::enter(self, id, value, std::time::Duration::from_secs(10)).await
    }

    /// Sends the public editable-text command; the caller must verify the read-back.
    async fn set_text(&self, node: &Node, value: &str) -> Result<(), AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.set_text_once(&node, value).await },
        )
        .await
    }

    async fn set_text_once(&self, node: &Node, value: &str) -> Result<(), AccessError> {
        let proxy = EditableTextProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(classify)?;
        if proxy.set_text_contents(value).await.map_err(classify)? {
            Ok(())
        } else {
            Err(error("text entry refused").into())
        }
    }

    /// Reads text content when the public Text interface is implemented.
    pub async fn text_value(&self, node: &Node) -> Result<String, AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.text_value_once(&node).await },
        )
        .await
    }

    async fn text_value_once(&self, node: &Node) -> Result<String, AccessError> {
        if !self
            .proxy(node)
            .await?
            .get_interfaces()
            .await
            .map_err(classify)?
            .contains(atspi::Interface::Text)
        {
            return Ok(label(&node.name)
                .map(|(_, value)| value.to_owned())
                .unwrap_or_default());
        }
        TextProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(classify)?
            .get_text(0, -1)
            .await
            .map_err(classify)
    }

    async fn focus_text(&self, node: &Node) -> Result<(), AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.focus_text_once(&node).await },
        )
        .await
    }

    async fn focus_text_once(&self, node: &Node) -> Result<(), AccessError> {
        let proxy = ActionProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(classify)?;
        let count = proxy.n_actions().await.map_err(classify)?;
        if !(0..=64).contains(&count) {
            return Err(error("invalid accessibility action count").into());
        }
        for index in 0..count {
            if proxy.get_name(index).await.map_err(classify)? == "ShowOnScreen"
                && !proxy.do_action(index).await.map_err(classify)?
            {
                return Err(error("text scroll action refused").into());
            }
        }
        for index in 0..count {
            if proxy
                .get_name(index)
                .await
                .map_err(classify)?
                .eq_ignore_ascii_case("tap")
            {
                return if proxy.do_action(index).await.map_err(classify)? {
                    Ok(())
                } else {
                    Err(error("text focus action refused").into())
                };
            }
        }
        Err(error("text control exposes no public Tap action").into())
    }

    /// Public keyboard-focus state used for editable controls and evidence.
    pub async fn focused(&self, node: &Node) -> Result<bool, AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.focused_once(&node).await },
        )
        .await
    }

    async fn focused_once(&self, node: &Node) -> Result<bool, AccessError> {
        Ok(self
            .proxy(node)
            .await?
            .get_state()
            .await
            .map_err(classify)?
            .contains(atspi::State::Focused))
    }

    /// The accessible enabled state.
    pub async fn enabled(&self, node: &Node) -> Result<bool, AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move { self.enabled_once(&node).await },
        )
        .await
    }

    async fn enabled_once(&self, node: &Node) -> Result<bool, AccessError> {
        Ok(self
            .proxy(node)
            .await?
            .get_state()
            .await
            .map_err(classify)?
            .contains(atspi::State::Enabled))
    }
}

#[async_trait::async_trait]
impl crate::text_entry::TextEntry for Native {
    async fn find(&self, id: &str) -> Result<Option<Node>, AppiumError> {
        Native::find(self, id).await
    }
    async fn focus(&self, node: &Node) -> Result<(), AppiumError> {
        self.focus_text(node).await
    }
    async fn focused(&self, node: &Node) -> Result<bool, AppiumError> {
        Native::focused(self, node).await
    }
    async fn set(&self, node: &Node, value: &str) -> Result<(), AppiumError> {
        self.set_text(node, value).await
    }
    async fn text(&self, node: &Node) -> Result<String, AppiumError> {
        with_fresh_node(
            node,
            Duration::from_secs(10),
            |id| async move { self.find(&id).await },
            |node| async move {
                let interfaces = self
                    .proxy(&node)
                    .await?
                    .get_interfaces()
                    .await
                    .map_err(classify)?;
                if !interfaces.contains(atspi::Interface::EditableText)
                    || !interfaces.contains(atspi::Interface::Text)
                {
                    return Err(
                        error("text control lacks public editable and text interfaces").into(),
                    );
                }
                self.text_value_once(&node).await
            },
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn finds_a_unique_contract_line_after_flutter_tab_position_semantics() {
        assert_eq!(
            label("Tab 2 of 2\n[mortsom:order.book.tab.sell]\nSell BTC"),
            Some(("order.book.tab.sell", "\nSell BTC"))
        );
        assert_eq!(label("copy [mortsom:order.book.tab.sell]"), None);
        assert_eq!(label("[mortsom:first]\n[mortsom:second]"), None);
        assert_eq!(label("[mortsom:]"), None);
    }
    #[test]
    fn preserves_url_characters_in_dynamic_relay_identifiers() {
        assert_eq!(
            label("[mortsom:settings.relays.item.ws://127.0.0.1:7000]"),
            Some(("settings.relays.item.ws://127.0.0.1:7000", ""))
        );
    }
    #[test]
    fn identifies_exact_linux_contract_without_localized_copy() {
        assert_eq!(
            label("[mortsom:order.status]active"),
            Some(("order.status", "active"))
        );
        assert_eq!(label("Release"), None);
        assert_eq!(label("[mortsom:order.status"), None);
    }
}
