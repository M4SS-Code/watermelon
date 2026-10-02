# watermelon-testkit

Run tests against isolated, disposable `nats-server` instances.

Every test gets its own server, bound to random ports on `127.0.0.1` and
backed by a private temporary directory, so tests can run in parallel
without sharing any state.

```rust
use watermelon_testkit::NatsServer;

#[tokio::test]
async fn my_test() {
    let Some(server) = NatsServer::builder().jetstream().start().await else {
        // nats-server isn't installed: skip the test
        return;
    };

    // connect any NATS client to `server.client_url()`
}
```

The `nats-server` binary is taken from the `NATS_SERVER_BIN` environment
variable, or looked up in `PATH`. If it can't be found tests are skipped,
unless the `CI` environment variable is set, in which case they fail.

When a test fails the tail of the server's trace log is printed, and its
temporary directory, containing the configuration and the full log, is kept
for inspection.
