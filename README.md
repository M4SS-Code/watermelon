<h1 align="center">watermelon</h1>
<div align="center">
    <small>
        Pure Rust NATS client implementation and tokio integration
    </small>
</div>

`watermelon` is an independent implementation of the NATS client protocol
and the NATS client API for Rust. The goal of the project is to produce
an opinionated, composable, idiomatic implementation with a keen eye on
security, correctness and ease of use.

Watermelon is divided into multiple crates, all hosted in the same monorepo.

| Crate name         | Crates.io release                                                                                               | Docs                                                                                    | Description                                                               |
| ------------------ | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `watermelon`       | [![crates.io](https://img.shields.io/crates/v/watermelon.svg)](https://crates.io/crates/watermelon)             | [![Docs](https://docs.rs/watermelon/badge.svg)](https://docs.rs/watermelon)             | High level actor based NATS Core and NATS Jetstream client implementation |
| `watermelon-mini`  | [![crates.io](https://img.shields.io/crates/v/watermelon-mini.svg)](https://crates.io/crates/watermelon-mini)   | [![Docs](https://docs.rs/watermelon-mini/badge.svg)](https://docs.rs/watermelon-mini)   | Minimal NATS Core client implementation                                   |
| `watermelon-net`   | [![crates.io](https://img.shields.io/crates/v/watermelon-net.svg)](https://crates.io/crates/watermelon-net)     | [![Docs](https://docs.rs/watermelon-net/badge.svg)](https://docs.rs/watermelon-net)     | Low-level NATS Core network implementation                                |
| `watermelon-proto` | [![crates.io](https://img.shields.io/crates/v/watermelon-proto.svg)](https://crates.io/crates/watermelon-proto) | [![Docs](https://docs.rs/watermelon-proto/badge.svg)](https://docs.rs/watermelon-proto) | `#[no_std]` NATS Core Sans-IO protocol implementation                     |
| `watermelon-nkeys` | [![crates.io](https://img.shields.io/crates/v/watermelon-nkeys.svg)](https://crates.io/crates/watermelon-nkeys) | [![Docs](https://docs.rs/watermelon-nkeys/badge.svg)](https://docs.rs/watermelon-nkeys) | Minimal NKeys implementation for NATS client authentication               |

# Advantages over `async-nats`

1. **Security**: this client is protected against command injection attacks via checked APIs like `Subject`.
2. **Extendibility**: exposes the inner components via `watermelon-mini` and `watermelon-net`.
3. **Error handling**: `subscribe` errors are correctly caught - internally enables server verbose mode.
4. **Fresh start**: this client only supports nats-server >=2.10.0. We may drop support for older server versions as new ones come out. `tls://` also uses _TLS-first handshake_ mode and doesn't support the older `STARTTLS`-style mode.
5. **Licensing**: dual licensed under MIT and APACHE-2.0.

# Disadvantages over `async-nats`

1. **Completeness**: most APIs (Jetstream specifically) haven't been implemented yet.
2. **Future work**: we may never add support for functionallity that we, M4SS Srl, don't use.
3. **Difference in APIs**: official NATS clients tend to have similar APIs. `watermelon` does not follow the official guidelines.
4. **Ecosystem**: as the client does not support older server versions or ignores old configuration options, it may not work in an environment that hasn't adopted the new standards yet.
5. **Backwards compatibility**: this client is in no way compatible with the `async-nats` API and may make frequent breaking changes.

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
