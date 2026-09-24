#![allow(clippy::unwrap_used)]
use super::*;
use atspi::zbus::zvariant::OwnedObjectPath;

type Reference = (String, OwnedObjectPath);

fn reference(path: &str) -> Reference {
    (":1.1".into(), OwnedObjectPath::try_from(path).unwrap())
}

/// An accessible as at-spi2-atk 2.38 serves it: an occupied socket (Flutter's
/// view) counts and indexes its embedded plug, but leaves it out of
/// GetChildren.
struct Accessible {
    name: String,
    children: Vec<Reference>,
    embedded: Option<Reference>,
}

#[zbus::interface(name = "org.a11y.atspi.Accessible")]
impl Accessible {
    #[zbus(property)]
    fn name(&self) -> String {
        self.name.clone()
    }
    #[zbus(property)]
    fn child_count(&self) -> i32 {
        (self.children.len() + usize::from(self.embedded.is_some())) as i32
    }
    fn get_children(&self) -> Vec<Reference> {
        self.children.clone()
    }
    fn get_child_at_index(&self, index: i32) -> Reference {
        self.embedded
            .iter()
            .chain(&self.children)
            .nth(index as usize)
            .cloned()
            .unwrap_or_else(|| reference("/org/a11y/atspi/null"))
    }
}

async fn serve(objects: Vec<(&str, Accessible)>) -> (Connection, Connection) {
    let (client, server) = tokio::net::UnixStream::pair().unwrap();
    let guid = zbus::Guid::generate();
    let mut builder = zbus::connection::Builder::unix_stream(server)
        .server(guid)
        .unwrap()
        .p2p();
    for (path, object) in objects {
        builder = builder.serve_at(path, object).unwrap();
    }
    let client = zbus::connection::Builder::unix_stream(client).p2p().build();
    let (server, client) = tokio::try_join!(builder.build(), client).unwrap();
    (server, client)
}

fn object(name: &str, children: &[&str], embedded: Option<&str>) -> Accessible {
    Accessible {
        name: name.into(),
        children: children.iter().map(|path| reference(path)).collect(),
        embedded: embedded.map(reference),
    }
}

#[tokio::test]
async fn reads_the_flutter_tree_embedded_in_an_accessibility_socket() {
    let (_server, client) = serve(vec![
        (
            "/org/a11y/atspi/accessible/root",
            object("main", &["/app"], None),
        ),
        ("/app", object("foundation.mostro.app", &["/view"], None)),
        ("/view", object("", &[], Some("/plug"))),
        ("/plug", object("", &["/marker"], None)),
        ("/marker", object("[mortsom:env.marker]TEST", &[], None)),
    ])
    .await;
    let native = Native::from_connection(client);
    let marker = native.find("env.marker").await.unwrap();
    assert_eq!(marker.map(|node| node.path), Some("/marker".into()));
}
