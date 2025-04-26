use alloc::boxed::Box;
use core::num::NonZero;

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
        max_messages: Option<NonZero<u64>>,
    },
    Ping,
    Pong,
}
