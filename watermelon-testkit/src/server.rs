use std::{
    env, fmt,
    net::SocketAddr,
    path::{Path, PathBuf},
    process::Stdio,
    thread,
    time::Duration,
};

use hyper::StatusCode;
use rustix::process::{Pid, Signal, kill_process};
use serde_json::Value;
use tempfile::TempDir;
use tokio::{
    fs,
    process::{Child, Command},
    time::{Instant, sleep},
};

use crate::http::HttpClient;

const STARTUP_TIMEOUT: Duration = Duration::from_secs(15);
const LOG_TAIL_LINES: usize = 50;
const NOT_FOUND: &str =
    "nats-server binary not found; set NATS_SERVER_BIN or add nats-server to PATH";

/// Configuration for a [`NatsServer`]
///
/// Obtained from [`NatsServer::builder`].
#[derive(Debug, Default)]
#[must_use]
pub struct NatsServerBuilder {
    jetstream: bool,
    jetstream_domain: Option<String>,
    websocket: bool,
    users: Vec<(String, String)>,
    max_payload: Option<u32>,
    extra_config: Vec<String>,
}

/// A `nats-server` process owned by a single test
///
/// The process is killed when this is dropped. If the test is panicking,
/// the tail of the server log is printed and the temporary directory
/// holding the config, the full log and the `JetStream` store is kept
/// around for inspection.
#[derive(Debug)]
pub struct NatsServer {
    child: Child,
    pid: Pid,
    dir: TempDir,
    client_url: String,
    websocket_url: Option<String>,
    monitoring_addr: SocketAddr,
    http: HttpClient,
}

impl NatsServerBuilder {
    /// Enable `JetStream`, storing data in the server's temporary directory
    pub fn jetstream(mut self) -> Self {
        self.jetstream = true;
        self
    }

    /// Enable `JetStream` under the given `domain`
    pub fn jetstream_domain(mut self, domain: impl Into<String>) -> Self {
        self.jetstream = true;
        self.jetstream_domain = Some(domain.into());
        self
    }

    /// Enable a plaintext WebSocket listener
    pub fn websocket(mut self) -> Self {
        self.websocket = true;
        self
    }

    /// Require clients to authenticate, allowing `username` with `password`
    ///
    /// Can be called multiple times to allow multiple users.
    pub fn user(mut self, username: impl Into<String>, password: impl Into<String>) -> Self {
        self.users.push((username.into(), password.into()));
        self
    }

    /// Set the maximum payload size accepted by the server
    pub fn max_payload(mut self, max_payload: u32) -> Self {
        self.max_payload = Some(max_payload);
        self
    }

    /// Append raw lines to the generated server configuration file
    pub fn config(mut self, config: impl Into<String>) -> Self {
        self.extra_config.push(config.into());
        self
    }

    /// Start the server, waiting until it's ready to accept clients
    ///
    /// Returns `None` if no `nats-server` binary could be found
    /// and the `CI` environment variable isn't set.
    ///
    /// # Panics
    ///
    /// Panics if the binary is missing while running in CI,
    /// or if the server fails to start.
    #[must_use]
    pub async fn start(self) -> Option<NatsServer> {
        let Some(bin) = find_nats_server() else {
            assert!(
                env::var_os("CI").is_none_or(|ci| ci.is_empty()),
                "{NOT_FOUND}"
            );
            eprintln!("SKIPPED: {NOT_FOUND}");
            return None;
        };

        let dir = tempfile::Builder::new()
            .prefix("watermelon-testkit-")
            .tempdir()
            .expect("create server temporary directory");
        let config_path = dir.path().join("nats-server.conf");
        fs::write(&config_path, self.render(dir.path()))
            .await
            .expect("write server config");

        let child = Command::new(&bin)
            .arg("--config")
            .arg(&config_path)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true)
            .spawn()
            .unwrap_or_else(|err| panic!("spawn {}: {err}", bin.display()));
        let pid = child
            .id()
            .and_then(|id| Pid::from_raw(id.try_into().ok()?))
            .expect("nats-server pid");

        let mut server = NatsServer {
            child,
            pid,
            dir,
            client_url: String::new(),
            websocket_url: None,
            monitoring_addr: SocketAddr::from(([0, 0, 0, 0], 0)),
            http: HttpClient::new(),
        };
        server.wait_ready(self.jetstream).await;
        Some(server)
    }

    fn render(&self, dir: &Path) -> String {
        let mut config = String::new();
        self.write_config(&mut config, dir)
            .expect("formatting into a String never fails");
        config
    }

    fn write_config(&self, w: &mut impl fmt::Write, dir: &Path) -> fmt::Result {
        writeln!(w, r#"listen: "127.0.0.1:-1""#)?;
        writeln!(w, r#"http: "127.0.0.1:-1""#)?;
        writeln!(w, "ports_file_dir: {}", quote(dir))?;
        writeln!(w, "log_file: {}", quote(dir.join("nats-server.log")))?;
        writeln!(w, "logtime: true")?;
        writeln!(w, "debug: true")?;
        writeln!(w, "trace: true")?;
        writeln!(w, "max_traced_msg_len: 256")?;

        if self.jetstream {
            write!(
                w,
                "jetstream {{ store_dir: {}",
                quote(dir.join("jetstream"))
            )?;
            if let Some(domain) = &self.jetstream_domain {
                write!(w, ", domain: {}", quote(domain))?;
            }
            writeln!(w, " }}")?;
        }
        if self.websocket {
            writeln!(w, r#"websocket {{ listen: "127.0.0.1:-1", no_tls: true }}"#)?;
        }
        if !self.users.is_empty() {
            write!(w, "authorization {{ users: [")?;
            for (username, password) in &self.users {
                write!(
                    w,
                    " {{ user: {}, password: {} }}",
                    quote(username),
                    quote(password)
                )?;
            }
            writeln!(w, " ] }}")?;
        }
        if let Some(max_payload) = self.max_payload {
            writeln!(w, "max_payload: {max_payload}")?;
        }
        for extra in &self.extra_config {
            writeln!(w, "{extra}")?;
        }

        Ok(())
    }
}

impl NatsServer {
    /// Configure a new server
    pub fn builder() -> NatsServerBuilder {
        NatsServerBuilder::default()
    }

    /// The `nats://` URL clients should connect to
    #[must_use]
    pub fn client_url(&self) -> &str {
        &self.client_url
    }

    /// The `ws://` URL clients should connect to
    ///
    /// Returns `None` unless [`NatsServerBuilder::websocket`] was called.
    #[must_use]
    pub fn websocket_url(&self) -> Option<&str> {
        self.websocket_url.as_deref()
    }

    /// Query a monitoring endpoint, like `/connz?subs=1` or `/varz`
    ///
    /// # Panics
    ///
    /// Panics if the request fails or the response isn't successful JSON.
    #[must_use]
    pub async fn monitor(&self, path_and_query: &str) -> Value {
        let (status, body) = self
            .http
            .get(self.monitoring_addr, path_and_query)
            .await
            .unwrap_or_else(|err| panic!("GET {path_and_query}: {err}"));
        assert_eq!(
            status,
            StatusCode::OK,
            "GET {path_and_query}: {}",
            String::from_utf8_lossy(&body)
        );
        serde_json::from_slice(&body)
            .unwrap_or_else(|err| panic!("GET {path_and_query}: invalid JSON: {err}"))
    }

    /// The connections currently open with the server, including their subscriptions
    ///
    /// Shorthand for the `connections` array of `/connz?subs=1`.
    #[must_use]
    pub async fn connections(&self) -> Vec<Value> {
        match self
            .monitor("/connz?subs=1")
            .await
            .get_mut("connections")
            .map(Value::take)
        {
            Some(Value::Array(connections)) => connections,
            _ => Vec::new(),
        }
    }

    /// Put the server into Lame Duck Mode
    pub fn lame_duck(&self) {
        self.signal(Signal::USR2);
    }

    /// Freeze the server process, leaving its sockets open
    pub fn pause(&self) {
        self.signal(Signal::STOP);
    }

    /// Resume a server frozen by [`NatsServer::pause`]
    pub fn resume(&self) {
        self.signal(Signal::CONT);
    }

    /// Gracefully shut the server down and wait for it to exit
    ///
    /// # Panics
    ///
    /// Panics if waiting on the process fails.
    pub async fn shutdown(&mut self) {
        self.signal(Signal::TERM);
        self.child
            .wait()
            .await
            .expect("wait for nats-server to exit");
    }

    /// The full server log
    pub async fn log(&self) -> String {
        fs::read_to_string(self.log_path())
            .await
            .unwrap_or_default()
    }

    fn log_path(&self) -> PathBuf {
        self.dir.path().join("nats-server.log")
    }

    fn signal(&self, signal: Signal) {
        kill_process(self.pid, signal)
            .unwrap_or_else(|err| panic!("send {signal:?} to nats-server: {err}"));
    }

    async fn wait_ready(&mut self, jetstream: bool) {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        let ports_path = self
            .dir
            .path()
            .join(format!("nats-server_{}.ports", self.pid.as_raw_nonzero()));

        // The ports file is written once all listeners are bound. It may be
        // observed half-written, so retry until it parses.
        let ports = loop {
            if let Some(ports) = fs::read_to_string(&ports_path)
                .await
                .ok()
                .and_then(|ports| serde_json::from_str::<Value>(&ports).ok())
            {
                break ports;
            }
            self.assert_starting(deadline).await;
        };

        let first_url = |kind: &str| ports[kind][0].as_str().map(str::to_owned);
        self.client_url = first_url("nats").expect("ports file without a nats listener");
        self.websocket_url = first_url("websocket");
        self.monitoring_addr = first_url("monitoring")
            .and_then(|url| url.strip_prefix("http://")?.parse().ok())
            .expect("ports file without a monitoring listener");

        let healthz = if jetstream {
            "/healthz?js-enabled-only=true"
        } else {
            "/healthz"
        };
        while !matches!(
            self.http.get(self.monitoring_addr, healthz).await,
            Ok((StatusCode::OK, _))
        ) {
            self.assert_starting(deadline).await;
        }
    }

    async fn assert_starting(&mut self, deadline: Instant) {
        if let Some(status) = self.child.try_wait().expect("poll nats-server") {
            panic!("nats-server exited during startup with {status}");
        }
        assert!(
            Instant::now() < deadline,
            "nats-server did not become ready within {STARTUP_TIMEOUT:?}"
        );
        sleep(Duration::from_millis(10)).await;
    }
}

impl Drop for NatsServer {
    fn drop(&mut self) {
        // The process is reaped in the background by tokio
        let _ = self.child.start_kill();

        if thread::panicking() {
            self.dir.disable_cleanup(true);
            let log = std::fs::read_to_string(self.log_path()).unwrap_or_default();
            let lines = log.lines().collect::<Vec<_>>();
            let tail = &lines[lines.len().saturating_sub(LOG_TAIL_LINES)..];
            eprintln!(
                "---- nats-server log tail (full log and config kept in {}) ----\n{}",
                self.dir.path().display(),
                tail.join("\n")
            );
        }
    }
}

fn find_nats_server() -> Option<PathBuf> {
    if let Some(bin) = env::var_os("NATS_SERVER_BIN")
        .map(PathBuf::from)
        .filter(|bin| bin.is_file())
    {
        return Some(bin);
    }

    env::split_paths(&env::var_os("PATH")?)
        .map(|dir| dir.join("nats-server"))
        .find(|bin| bin.is_file())
}

/// Quote a value for the server config file, which accepts JSON strings
fn quote(value: impl AsRef<Path>) -> String {
    let value = value.as_ref().to_str().expect("non UTF-8 path");
    serde_json::to_string(value).expect("serialize string")
}
