//! Linux accessibility traversal and actions over an actor-owned AT-SPI bus.
use crate::native_retry::{AccessError, classify, retry, with_fresh_node};
use atspi::proxy::{
    accessible::AccessibleProxy, action::ActionProxy, editable_text::EditableTextProxy,
    text::TextProxy,
};
use atspi::zbus::{self, Connection};
use futures::future::try_join_all;
use mortsom_ui_driver::error::UiError;
use mortsom_ui_driver::webdriver::WebDriverError;
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

/// Most nodes a walk reads before it refuses the tree.
const NODE_LIMIT: usize = 5000;

/// A node's name and children, asked together. Flutter's view is an
/// accessibility socket, and at-spi2-atk before its merge into at-spi2-core
/// (2.38, Ubuntu 22.04) leaves the socket's embedded tree out of
/// GetChildren: a list shorter than the child count is read by index
/// instead, as libatspi does.
async fn read_node(proxy: &AccessibleProxy<'_>) -> Result<(String, Vec<Node>), AccessError> {
    let (name, count, listed) =
        tokio::join!(proxy.name(), proxy.child_count(), proxy.get_children());
    let (name, count) = (name.map_err(classify)?, count.map_err(classify)?);
    let mut children = listed.map_err(classify)?;
    if usize::try_from(count).ok() != Some(children.len()) {
        children = try_join_all((0..count).map(|index| proxy.get_child_at_index(index)))
            .await
            .map_err(classify)?;
    }
    let children = children
        .iter()
        .filter_map(|child| {
            child.name_as_str().map(|destination| Node {
                destination: destination.into(),
                path: child.path_as_str().into(),
                name: String::new(),
            })
        })
        .collect();
    Ok((name, children))
}

/// Puts the nodes read level by level, each with the indexes of its
/// children, in the order of a depth-first walk that takes the last child
/// first: the order `find` and the evidence have always read them in.
fn depth_first(visited: Vec<(Node, Vec<usize>)>) -> Vec<Node> {
    let mut order = Vec::with_capacity(visited.len());
    let mut pending = vec![0];
    while let Some(index) = pending.pop() {
        if let Some((_, children)) = visited.get(index) {
            order.push(index);
            pending.extend(children);
        }
    }
    let mut nodes: Vec<Option<Node>> = visited.into_iter().map(|(node, _)| Some(node)).collect();
    order
        .into_iter()
        .filter_map(|index| nodes.get_mut(index).and_then(Option::take))
        .collect()
}

/// Converts transport errors without exposing application data.
pub fn error(cause: impl std::fmt::Display) -> UiError {
    UiError::WebDriver(WebDriverError::Transport(
        mortsom_common::Redactor::default().redact(&format!("Linux accessibility: {cause}")),
    ))
}

/// Extracts the node's contract line, allowing Flutter's merged tab-position label.
///
/// Flutter writes a merged node's own label before the labels it absorbs, so
/// a name that starts with a contract line is that line's node even when a
/// child's line follows (a node card merged into the first run's node
/// choice); its value stops where the child's line starts, and the child's
/// identifier, absorbed into it, is not a node of its own to find. A
/// contract line anywhere else must be the only one, or the name is
/// ambiguous.
pub fn label(name: &str) -> Option<(&str, &str)> {
    let mut markers = name
        .match_indices("[mortsom:")
        .filter(|(index, _)| *index == 0 || name.as_bytes()[index - 1] == b'\n');
    let (index, _) = markers.next()?;
    let next = markers.next().map(|(next, _)| next);
    if index != 0 && next.is_some() {
        return None;
    }
    let own = &name[index..next.map_or(name.len(), |next| next - 1)];
    let (id, value) = own.strip_prefix("[mortsom:")?.split_once(']')?;
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
    pub async fn connect(address: &str) -> Result<Self, UiError> {
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
                    return Ok::<_, UiError>(());
                }
            }
        })
        .await
        .map_err(error)??;
        drop(proxy);
        Ok(Self { connection })
    }

    #[cfg(test)]
    fn from_connection(connection: Connection) -> Self {
        Self { connection }
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
    pub async fn nodes(&self) -> Result<Vec<Node>, UiError> {
        retry(Duration::from_secs(10), || self.nodes_once()).await
    }

    /// Reads a level of the tree at a time, all of its calls out at once. A
    /// busy app answers one call per turn of its main loop, so a walk that
    /// waited for each answer before the next call paid a turn per call.
    async fn nodes_once(&self) -> Result<Vec<Node>, AccessError> {
        let mut visited: Vec<(Node, Vec<usize>)> = Vec::new();
        // Each node waits beside the index of the parent that listed it.
        let mut level: Vec<(Option<usize>, Node)> = vec![(
            None,
            Node {
                destination: "org.a11y.atspi.Registry".into(),
                path: "/org/a11y/atspi/accessible/root".into(),
                name: String::new(),
            },
        )];
        while !level.is_empty() {
            if visited.len() + level.len() > NODE_LIMIT {
                return Err(error("accessibility tree exceeds limit").into());
            }
            let answers = try_join_all(
                level
                    .iter()
                    .map(|(_, node)| async move { read_node(&self.proxy(node).await?).await }),
            )
            .await?;
            let mut next = Vec::new();
            for ((parent, node), (name, children)) in level.into_iter().zip(answers) {
                let index = visited.len();
                if let Some((_, of_parent)) = parent.and_then(|parent| visited.get_mut(parent)) {
                    of_parent.push(index);
                }
                visited.push((Node { name, ..node }, Vec::new()));
                next.extend(children.into_iter().map(|child| (Some(index), child)));
            }
            level = next;
        }
        Ok(depth_first(visited))
    }

    /// Finds an exact namespaced identifier, never localized copy.
    pub async fn find(&self, id: &str) -> Result<Option<Node>, UiError> {
        let nodes = tokio::time::timeout(std::time::Duration::from_secs(10), self.nodes())
            .await
            .map_err(error)??;
        Ok(nodes
            .into_iter()
            .find(|node| label(&node.name).is_some_and(|(key, _)| key == id)))
    }

    /// Invokes the accessible object's default user action.
    pub async fn click(&self, node: &Node) -> Result<(), UiError> {
        self.click_within(node, Duration::from_secs(10)).await
    }

    /// `timeout` bounds the wait for a control that takes actions, never the
    /// press: one cut off after the app received it may still run there.
    /// Once DoAction starts, every error is final: replaying a business
    /// action after a lost acknowledgement could submit twice.
    async fn click_within(&self, node: &Node, timeout: Duration) -> Result<(), UiError> {
        let proxy = with_fresh_node(
            node,
            timeout,
            |id| async move { self.find(&id).await },
            |node| async move { self.actionable(node).await },
        )
        .await?;
        if proxy.do_action(0).await.map_err(error)? {
            Ok(())
        } else {
            Err(error("action refused"))
        }
    }

    /// The control's actions, once it takes one. A read-only check may
    /// detect a rebuilt node, or a control the app has not enabled yet,
    /// which exposes no action: both are found again.
    async fn actionable(&self, node: Node) -> Result<ActionProxy<'static>, AccessError> {
        let proxy = ActionProxy::builder(&self.connection)
            .destination(node.destination)
            .map_err(error)?
            .path(node.path)
            .map_err(error)?
            .build()
            .await
            .map_err(classify)?;
        if proxy.n_actions().await.map_err(classify)? <= 0 {
            return Err(AccessError::NotYet(error(
                "control exposes no default user action",
            )));
        }
        Ok(proxy)
    }

    /// Focuses and verifies text through public accessibility state.
    pub async fn type_text(&self, node: &Node, value: &str) -> Result<(), UiError> {
        let (id, _) =
            label(&node.name).ok_or_else(|| error("text control has no contract identifier"))?;
        crate::text_entry::enter(self, id, value, std::time::Duration::from_secs(10)).await
    }

    /// Sends the public editable-text command; the caller must verify the read-back.
    async fn set_text(&self, node: &Node, value: &str) -> Result<(), UiError> {
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
    pub async fn text_value(&self, node: &Node) -> Result<String, UiError> {
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

    async fn focus_text(&self, node: &Node) -> Result<(), UiError> {
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
    pub async fn focused(&self, node: &Node) -> Result<bool, UiError> {
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
    pub async fn enabled(&self, node: &Node) -> Result<bool, UiError> {
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
    async fn find(&self, id: &str) -> Result<Option<Node>, UiError> {
        Native::find(self, id).await
    }
    async fn focus(&self, node: &Node) -> Result<(), UiError> {
        self.focus_text(node).await
    }
    async fn focused(&self, node: &Node) -> Result<bool, UiError> {
        Native::focused(self, node).await
    }
    async fn set(&self, node: &Node, value: &str) -> Result<(), UiError> {
        self.set_text(node, value).await
    }
    async fn text(&self, node: &Node) -> Result<String, UiError> {
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
        assert_eq!(label("Tab 2 of 2\n[mortsom:first]\n[mortsom:second]"), None);
        assert_eq!(label("[mortsom:]"), None);
    }

    /// The first run's node choice wraps each node card (`node.item.<pk>`)
    /// in its own merged `onboarding.community.card.<pk>`: the merged node
    /// carries its own line first and its child's after it.
    #[test]
    fn a_merged_node_is_the_identifier_its_name_starts_with() {
        assert_eq!(
            label("[mortsom:onboarding.community.card.ab]\n[mortsom:node.item.ab]\nMostro"),
            Some(("onboarding.community.card.ab", ""))
        );
        assert_eq!(
            label("[mortsom:first]value\n[mortsom:second]"),
            Some(("first", "value"))
        );
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

#[cfg(test)]
#[path = "native_tree_tests.rs"]
mod tree_tests;
