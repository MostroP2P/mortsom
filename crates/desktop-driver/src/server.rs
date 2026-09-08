//! Loopback-only Flutter Web bundle server with required isolation headers.
use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

/// Owns one static server and every request task.
pub struct WebServer {
    url: String,
    task: Option<tokio::task::JoinHandle<std::io::Result<()>>>,
    stop: Option<tokio::sync::oneshot::Sender<()>>,
}

impl WebServer {
    /// Serves this existing build on an ephemeral loopback port.
    pub async fn start(root: &Path) -> std::io::Result<Self> {
        Self::start_owned(root, Arc::new(())).await
    }

    /// Retains the caller's bundle owner until the last request has stopped.
    pub async fn start_owned<T: Send + Sync + 'static>(
        root: &Path,
        owner: Arc<T>,
    ) -> std::io::Result<Self> {
        let root = root.canonicalize()?;
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = format!("http://{}", listener.local_addr()?);
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(serve(listener, root, owner, receiver));
        Ok(Self {
            url,
            task: Some(task),
            stop: Some(stop),
        })
    }
    /// Local origin supplied to both independently isolated browsers.
    pub fn url(&self) -> &str {
        &self.url
    }

    /// Stops all requests before releasing the server.
    pub async fn stop(mut self) -> std::io::Result<()> {
        self.shutdown().await
    }

    /// Retains the join handle if the caller cancels shutdown and retries it.
    pub async fn shutdown(&mut self) -> std::io::Result<()> {
        self.signal_stop();
        let result = match self.task.as_mut() {
            Some(task) => task
                .await
                .unwrap_or_else(|cause| Err(std::io::Error::other(cause))),
            None => return Ok(()),
        };
        self.task = None;
        result
    }

    fn signal_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            // A closed receiver means the server already exited; stop() still joins its result.
            if stop.send(()).is_err() {
                tracing::debug!("Web server already stopped");
            }
        }
    }
}
impl Drop for WebServer {
    fn drop(&mut self) {
        self.signal_stop();
    }
}

async fn serve<T: Send + Sync + 'static>(
    listener: TcpListener,
    root: PathBuf,
    owner: Arc<T>,
    mut stop: tokio::sync::oneshot::Receiver<()>,
) -> std::io::Result<()> {
    let mut requests = tokio::task::JoinSet::new();
    let result = loop {
        tokio::select! {
            _ = &mut stop => break Ok(()),
            done = requests.join_next(), if !requests.is_empty() => {
                if let Some(Err(error)) = done { break Err(std::io::Error::other(error)); }
            },
            connection = listener.accept() => {
                let (stream, _) = match connection { Ok(value) => value, Err(error) => break Err(error) };
                let (root, owner) = (root.clone(), Arc::clone(&owner));
                requests.spawn(async move {
                    request(stream, root).await;
                    drop(owner);
                });
            },
        }
    };
    drop(listener);
    drain_requests(&mut requests, result).await
}

async fn drain_requests(
    requests: &mut tokio::task::JoinSet<()>,
    mut result: std::io::Result<()>,
) -> std::io::Result<()> {
    requests.abort_all();
    while let Some(done) = requests.join_next().await {
        if let Err(error) = done
            && !error.is_cancelled()
        {
            result = Err(std::io::Error::other(error));
        }
    }
    result
}

async fn request(stream: TcpStream, root: PathBuf) {
    match tokio::time::timeout(std::time::Duration::from_secs(30), respond(stream, root)).await {
        Ok(Ok(())) => {}
        Ok(Err(error)) => {
            tracing::debug!(kind = ?error.kind(), "Web request ended with an I/O error")
        }
        Err(_) => tracing::debug!("Web request deadline reached"),
    }
}

async fn respond(mut stream: TcpStream, root: PathBuf) -> std::io::Result<()> {
    let mut request = Vec::new();
    let mut chunk = [0u8; 2048];
    while !request.windows(4).any(|w| w == b"\r\n\r\n") && request.len() < 8192 {
        let len = stream.read(&mut chunk).await?;
        if len == 0 {
            return Ok(());
        }
        request.extend_from_slice(&chunk[..len]);
    }
    let line = String::from_utf8_lossy(&request);
    let mut tokens = line.split_whitespace();
    let method = tokens.next().unwrap_or("");
    let target = tokens
        .next()
        .unwrap_or("/")
        .split('?')
        .next()
        .unwrap_or("/");
    let path = root.join(if target == "/" {
        "index.html"
    } else {
        target.trim_start_matches('/')
    });
    let (status, body, content_type) = match path.canonicalize() {
        Ok(path)
            if path.starts_with(&root) && path.is_file() && matches!(method, "GET" | "HEAD") =>
        {
            let body = tokio::fs::read(&path).await?;
            ("200 OK", body, mime(&path))
        }
        _ => ("404 Not Found", b"Not found".to_vec(), "text/plain"),
    };
    let headers = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nCross-Origin-Opener-Policy: same-origin\r\nCross-Origin-Embedder-Policy: require-corp\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(headers.as_bytes()).await?;
    if method != "HEAD" {
        stream.write_all(&body).await?;
    }
    stream.shutdown().await
}

fn mime(path: &Path) -> &'static str {
    match path.extension().and_then(|s| s.to_str()).unwrap_or("") {
        "html" => "text/html",
        "js" | "mjs" => "application/javascript",
        "wasm" => "application/wasm",
        "json" => "application/json",
        "css" => "text/css",
        "png" => "image/png",
        "svg" => "image/svg+xml",
        "ttf" => "font/ttf",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    #[tokio::test]
    async fn cancelled_shutdown_retains_the_join_until_the_request_owner_finishes() {
        let (stop, receiver) = tokio::sync::oneshot::channel();
        let (finish, completion) = tokio::sync::oneshot::channel();
        let task = tokio::spawn(async move {
            let _ = receiver.await;
            completion.await.map_err(std::io::Error::other)
        });
        let mut server = super::WebServer {
            url: "http://127.0.0.1:9000".into(),
            task: Some(task),
            stop: Some(stop),
        };
        assert!(
            tokio::time::timeout(std::time::Duration::from_millis(10), server.shutdown())
                .await
                .is_err()
        );
        assert!(server.task.is_some());
        finish.send(()).unwrap();
        server.shutdown().await.unwrap();
        assert!(server.task.is_none());
    }

    use super::*;

    #[tokio::test]
    async fn a_request_panic_does_not_skip_draining_other_requests() {
        let mut requests = tokio::task::JoinSet::new();
        let panic = requests.spawn(async { panic!("injected request panic") });
        tokio::time::timeout(std::time::Duration::from_secs(1), async {
            while !panic.is_finished() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        requests.spawn(std::future::pending::<()>());
        assert!(drain_requests(&mut requests, Ok(())).await.is_err());
        assert!(
            requests.is_empty(),
            "shutdown returned before joining every request"
        );
    }

    #[tokio::test]
    async fn shutdown_closes_an_incomplete_request_without_waiting_for_its_deadline() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("index.html"), b"ready").unwrap();
        let owner = Arc::new(());
        let weak = Arc::downgrade(&owner);
        let server = WebServer::start_owned(dir.path(), owner).await.unwrap();
        let mut slow = TcpStream::connect(server.url().trim_start_matches("http://"))
            .await
            .unwrap();
        slow.write_all(b"GET / HTTP/1.1\r\n").await.unwrap();
        // A second completed request proves that the accept loop is running.
        assert!(
            reqwest::get(server.url())
                .await
                .unwrap()
                .status()
                .is_success()
        );
        assert!(weak.upgrade().is_some());
        server.stop().await.unwrap();
        assert!(
            weak.upgrade().is_none(),
            "bundle ownership survived request draining"
        );
        let mut byte = [0];
        let closed =
            tokio::time::timeout(std::time::Duration::from_secs(1), slow.read(&mut byte)).await;
        assert!(
            matches!(closed, Ok(Ok(0)) | Ok(Err(_))),
            "an accepted request survived server shutdown"
        );
    }
    #[tokio::test]
    async fn serves_wasm_with_isolation_and_refuses_paths_outside_the_bundle() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("core.wasm"), b"wasm").unwrap();
        let server = WebServer::start(dir.path()).await.unwrap();
        let response = reqwest::get(format!("{}/core.wasm", server.url()))
            .await
            .unwrap();
        assert_eq!(
            response.headers()["cross-origin-opener-policy"],
            "same-origin"
        );
        assert_eq!(
            response.headers()["cross-origin-embedder-policy"],
            "require-corp"
        );
        assert_eq!(response.headers()["content-type"], "application/wasm");
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"wasm");
        std::os::unix::fs::symlink("/etc/passwd", dir.path().join("outside")).unwrap();
        assert_eq!(
            reqwest::get(format!("{}/outside", server.url()))
                .await
                .unwrap()
                .status(),
            404
        );
    }
}
