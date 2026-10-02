use std::{
    sync::atomic::{AtomicUsize, Ordering},
    time::Duration,
};

use bytes::Bytes;
use futures_util::{FutureExt as _, StreamExt as _};
use watermelon::{
    core::{Client, ClientBuilder, Subscription, error::ResponseError},
    proto::{ServerAddr, ServerMessage, Subject},
};
use watermelon_testkit::{NatsServer, TimeoutExt as _};

/// How long a test waits on any single operation before failing
pub(crate) const TIMEOUT: Duration = Duration::from_secs(10);

/// Start a plain server, or return `None` to skip the test
pub(crate) async fn server() -> Option<NatsServer> {
    NatsServer::builder().start().await
}

pub(crate) fn server_addr(server: &NatsServer) -> ServerAddr {
    server.client_url().parse().expect("parse server URL")
}

pub(crate) async fn connect(server: &NatsServer) -> Client {
    connect_with(server, Client::builder()).await
}

pub(crate) async fn connect_with(server: &NatsServer, builder: ClientBuilder) -> Client {
    // Boxed because the connect future is large enough to trip `clippy::large_futures`
    Box::pin(builder.connect(server_addr(server)))
        .within(TIMEOUT)
        .await
        .expect("connect to nats-server")
}

pub(crate) async fn subscribe(client: &Client, subject: &str) -> Subscription {
    client
        .subscribe(subject.parse().expect("valid subject"), None)
        .within(TIMEOUT)
        .await
        .expect("subscribe")
}

pub(crate) async fn publish(client: &Client, subject: &str, payload: &'static [u8]) {
    client
        .publish(subject.parse().expect("valid subject"))
        .payload(payload.into())
        .within(TIMEOUT)
        .await
        .expect("publish");
}

/// Wait until the server has processed everything `client` sent before this call
///
/// The server handles the operations of a connection in order, and writes
/// to it in order. Sending a request nobody listens to and waiting for the
/// resulting "no responders" status therefore guarantees that:
///
/// * every previous `SUB`, `UNSUB` and `PUB` from `client` has been applied, and
/// * every message routed to `client` before that point has been dispatched
///   to its [`Subscription`].
pub(crate) async fn sync(client: &Client) {
    static NEXT: AtomicUsize = AtomicUsize::new(0);

    let subject = Subject::from_dangerous_value(
        format!("_SYNC.{}", NEXT.fetch_add(1, Ordering::Relaxed)).into(),
    );
    let response = client
        .request(subject)
        .payload(Bytes::new())
        .within(TIMEOUT)
        .await
        .expect("send sync request")
        .within(TIMEOUT)
        .await;
    assert!(
        matches!(response, Err(ResponseError::NoResponders)),
        "sync request unexpectedly answered: {response:?}"
    );
}

/// Receive the next message, failing on errors, timeouts and closed subscriptions
pub(crate) async fn next_message(subscription: &mut Subscription) -> ServerMessage {
    subscription
        .next()
        .within(TIMEOUT)
        .await
        .expect("subscription closed")
        .expect("subscription error")
}

/// Assert that nothing has been delivered to `subscription`
///
/// Only meaningful after [`sync`]ing every client that may have published
/// to it, followed by the client owning the subscription.
pub(crate) fn assert_no_message(subscription: &mut Subscription) {
    if let Some(message) = subscription.next().now_or_never() {
        panic!("unexpected message: {message:?}");
    }
}

/// The subjects `connection` (an entry of `/connz?subs=1`) is subscribed to
pub(crate) fn subscriptions(connection: &serde_json::Value) -> Vec<&str> {
    connection["subscriptions_list"]
        .as_array()
        .map(|subjects| subjects.iter().filter_map(|s| s.as_str()).collect())
        .unwrap_or_default()
}
