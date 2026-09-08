//! Linux accessibility traversal and actions over an actor-owned AT-SPI bus.
use atspi::proxy::{
    accessible::AccessibleProxy, action::ActionProxy, editable_text::EditableTextProxy,
    text::TextProxy,
};
use atspi::zbus::{self, Connection};
use mortsom_android_driver::appium::AppiumError;
use mortsom_android_driver::webdriver::WebDriverError;

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

/// Extracts the stable contract identifier and optional readout.
pub fn label(name: &str) -> Option<(&str, &str)> {
    name.strip_prefix("[mortsom:")?.split_once(']')
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

    async fn proxy<'a>(&'a self, node: &'a Node) -> Result<AccessibleProxy<'a>, AppiumError> {
        AccessibleProxy::builder(&self.connection)
            .cache_properties(zbus::proxy::CacheProperties::No)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(error)
    }

    /// Reads the current public tree, bounded by node count and time at caller.
    pub async fn nodes(&self) -> Result<Vec<Node>, AppiumError> {
        let mut pending = vec![Node {
            destination: "org.a11y.atspi.Registry".into(),
            path: "/org/a11y/atspi/accessible/root".into(),
            name: String::new(),
        }];
        let mut nodes = Vec::new();
        while let Some(mut node) = pending.pop() {
            if nodes.len() >= 5000 {
                return Err(error("accessibility tree exceeds limit"));
            }
            let proxy = self.proxy(&node).await?;
            let name = proxy
                .name()
                .await
                .map_err(|e| error(format!("read node {}: {e}", node.destination)))?;
            let children = proxy.get_children().await.map_err(error)?;
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
        let proxy = ActionProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(error)?;
        if proxy.do_action(0).await.map_err(error)? {
            Ok(())
        } else {
            Err(error("action refused"))
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
        let proxy = EditableTextProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(error)?;
        if proxy.set_text_contents(value).await.map_err(error)? {
            Ok(())
        } else {
            Err(error("text entry refused"))
        }
    }

    /// Reads text content when the public Text interface is implemented.
    pub async fn text_value(&self, node: &Node) -> Result<String, AppiumError> {
        if !self
            .proxy(node)
            .await?
            .get_interfaces()
            .await
            .map_err(error)?
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
            .map_err(error)?
            .get_text(0, -1)
            .await
            .map_err(error)
    }

    async fn focus_text(&self, node: &Node) -> Result<(), AppiumError> {
        let proxy = ActionProxy::builder(&self.connection)
            .destination(node.destination.as_str())
            .map_err(error)?
            .path(node.path.as_str())
            .map_err(error)?
            .build()
            .await
            .map_err(error)?;
        let count = proxy.n_actions().await.map_err(error)?;
        if !(0..=64).contains(&count) {
            return Err(error("invalid accessibility action count"));
        }
        for index in 0..count {
            if proxy.get_name(index).await.map_err(error)? == "ShowOnScreen"
                && !proxy.do_action(index).await.map_err(error)?
            {
                return Err(error("text scroll action refused"));
            }
        }
        for index in 0..count {
            if proxy
                .get_name(index)
                .await
                .map_err(error)?
                .eq_ignore_ascii_case("tap")
            {
                return if proxy.do_action(index).await.map_err(error)? {
                    Ok(())
                } else {
                    Err(error("text focus action refused"))
                };
            }
        }
        Err(error("text control exposes no public Tap action"))
    }

    /// Public keyboard-focus state used for editable controls and evidence.
    pub async fn focused(&self, node: &Node) -> Result<bool, AppiumError> {
        Ok(self
            .proxy(node)
            .await?
            .get_state()
            .await
            .map_err(error)?
            .contains(atspi::State::Focused))
    }

    /// The accessible enabled state.
    pub async fn enabled(&self, node: &Node) -> Result<bool, AppiumError> {
        Ok(self
            .proxy(node)
            .await?
            .get_state()
            .await
            .map_err(error)?
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
        let interfaces = self
            .proxy(node)
            .await?
            .get_interfaces()
            .await
            .map_err(error)?;
        if !interfaces.contains(atspi::Interface::EditableText)
            || !interfaces.contains(atspi::Interface::Text)
        {
            return Err(error(
                "text control lacks public editable and text interfaces",
            ));
        }
        self.text_value(node).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
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
