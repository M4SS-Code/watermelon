use alloc::boxed::Box;
use core::num::NonZeroU64;

use crate::{
    Subject, connect::Connect, message::MessageBase, queue_group::QueueGroup,
    subscription_id::SubscriptionId,
};

#[derive(Debug)]
pub enum ClientOp {
    Connect {
        connect: Box<Connect>,
    },
    Publish {
        message: MessageBase,
    },
    Subscribe {
        id: SubscriptionId,
        subject: Subject,
        queue_group: Option<QueueGroup>,
    },
    Unsubscribe {
        id: SubscriptionId,
        max_messages: Option<NonZeroU64>,
    },
    Ping,
    Pong,
}
