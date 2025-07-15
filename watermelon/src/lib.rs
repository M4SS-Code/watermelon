#![forbid(unsafe_code)]

pub use watermelon_proto as proto;

mod client;
mod handler;
mod multiplexed_subscription;
mod subscription;
#[cfg(test)]
pub(crate) mod tests;
mod util;

pub mod core {
    //! NATS Core functionality implementation

    pub use crate::client::{Client, ClientBuilder, Echo, QuickInfo};
    pub(crate) use crate::multiplexed_subscription::MultiplexedSubscription;
    pub use crate::subscription::Subscription;
    pub use watermelon_mini::AuthenticationMethod;

    pub mod publish {
        //! Utilities for publishing messages

        pub use crate::client::{
            ClientPublish, DoClientPublish, DoOwnedClientPublish, OwnedClientPublish, Publish,
            PublishBuilder,
        };
    }

    pub mod request {
        //! Utilities for publishing messages and awaiting for a response

        pub use crate::client::{
            ClientRequest, DoClientRequest, DoOwnedClientRequest, OwnedClientRequest, Request,
            RequestBuilder, ResponseFut,
        };
    }

    pub mod error {
        //! NATS Core specific errors

        pub use crate::{
            client::{ClientClosedError, ResponseError, TryCommandError},
            handler::ConnectHandlerError,
        };
        pub use watermelon_mini::ConnectError;
    }
}

pub mod jetstream {
    //! NATS Jetstream functionality implementation
    //!
    //! Relies on NATS Core to communicate with the NATS server

    pub use crate::client::{
        AckPolicy, Compression, Consumer, ConsumerBatch, ConsumerConfig, ConsumerDurability,
        ConsumerSpecificConfig, ConsumerStorage, ConsumerStream, Consumers, DeliverPolicy,
        DiscardPolicy, JetstreamClient, ReplayPolicy, RetentionPolicy, Storage, Stream,
        StreamConfig, StreamState, Streams,
    };

    pub mod error {
        //! NATS Jetstream specific errors

        pub use crate::client::{
            ConsumerStreamError, JetstreamApiError, JetstreamError, JetstreamErrorCode,
        };
    }
}
