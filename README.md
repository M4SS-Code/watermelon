<h1 align="center">watermelon</h1>
<div align="center">
    <small>
        Pure Rust NATS client implementation
    </small>
</div>

`watermelon` is an independent and opinionated implementation of the NATS
client protocol and the NATS client API for Rust. The goal of the project
is to produce a more secure, composable and idiomatic implementation compared
to the official one.

Most users of this project will depend on the `watermelon` crate directly and on
`watermelon-proto` and `watermelon-nkeys` via the re-exports in `watermelon`.

Watermelon is divided into multiple crates, all hosted in the same monorepo.

| Crate name         | Crates.io release                                                                                               | Docs                                                                                    | Description                                                               |
| ------------------ | --------------------------------------------------------------------------------------------------------------- | --------------------------------------------------------------------------------------- | ------------------------------------------------------------------------- |
| `watermelon`       | [![crates.io](https://img.shields.io/crates/v/watermelon.svg)](https://crates.io/crates/watermelon)             | [![Docs](https://docs.rs/watermelon/badge.svg)](https://docs.rs/watermelon)             | High level actor based NATS Core and NATS Jetstream client implementation |
| `watermelon-mini`  | [![crates.io](https://img.shields.io/crates/v/watermelon-mini.svg)](https://crates.io/crates/watermelon-mini)   | [![Docs](https://docs.rs/watermelon-mini/badge.svg)](https://docs.rs/watermelon-mini)   | Bare bones NATS Core client implementation                                |
| `watermelon-net`   | [![crates.io](https://img.shields.io/crates/v/watermelon-net.svg)](https://crates.io/crates/watermelon-net)     | [![Docs](https://docs.rs/watermelon-net/badge.svg)](https://docs.rs/watermelon-net)     | Low-level NATS Core network implementation                                |
| `watermelon-proto` | [![crates.io](https://img.shields.io/crates/v/watermelon-proto.svg)](https://crates.io/crates/watermelon-proto) | [![Docs](https://docs.rs/watermelon-proto/badge.svg)](https://docs.rs/watermelon-proto) | `#[no_std]` NATS Core Sans-IO protocol implementation                     |
| `watermelon-nkeys` | [![crates.io](https://img.shields.io/crates/v/watermelon-nkeys.svg)](https://crates.io/crates/watermelon-nkeys) | [![Docs](https://docs.rs/watermelon-nkeys/badge.svg)](https://docs.rs/watermelon-nkeys) | Minimal NKeys implementation for NATS client authentication               |

# Philosophy and Design

1. **Security by design**: this library uses type-safe and checked APIs, such as `Subject`, to prevent entire classes of errors and security vulnerabilities.
2. **Layering and composability**: the library is split into layers. You can get a high-level, batteries included implementation via `watermelon`, or depend directly on the lower-level crates for maximum flexibility.
3. **Opinionated, Rusty take**: we adapt the Go-style API of nats-server and apply different trade-offs to make NATS feel more Rusty. We sacrifice a bit of performance by enabling server verbose mode, and get better errors in return.
4. **Legacy is in the past**: we only support `nats-server >= 2.10` and avoid legacy versions compatibility code like the STARTTLS-style TLS upgrade path or fallbacks for older JetStream APIs. We also prefer pull consumers over push consumers given the robust flow control, easier compatibility with multi-account environments and stronger permissions handling.
5. **Permissive licensing**: dual licensed under MIT and APACHE-2.0.

## License

Licensed under either of

- Apache License, Version 2.0, ([LICENSE-APACHE](LICENSE-APACHE) or <https://www.apache.org/licenses/LICENSE-2.0>)
- MIT license ([LICENSE-MIT](LICENSE-MIT) or <https://opensource.org/licenses/MIT>)

at your option.

### Contribution

Unless you explicitly state otherwise, any contribution intentionally submitted for inclusion in the work by you, as defined in the Apache-2.0 license, shall be dual licensed as above, without any additional terms or conditions.
