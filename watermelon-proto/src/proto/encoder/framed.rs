use bytes::BytesMut;

use crate::proto::ClientOp;

use super::FrameEncoder;

#[derive(Debug)]
pub struct FramedEncoder {
    buf: BytesMut,
}

impl FramedEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            buf: BytesMut::new(),
        }
    }

    pub fn encode(&mut self, item: &ClientOp) -> BytesMut {
        struct Encoder<'a>(&'a mut FramedEncoder);

        impl FrameEncoder for Encoder<'_> {
            fn small_write(&mut self, buf: &[u8]) {
                self.0.buf.extend_from_slice(buf);
            }
        }

        super::encode(&mut Encoder(self), item);
        self.buf.split()
    }
}

impl Default for FramedEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZero;

    use bytes::Bytes;

    use super::FramedEncoder;
    use crate::{
        MessageBase, QueueGroup, Subject,
        headers::{HeaderMap, HeaderName, HeaderValue},
        proto::ClientOp,
        tests::ToBytes as _,
    };

    #[test]
    fn encode_ping() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder.encode(&ClientOp::Ping).to_bytes(),
            "PING\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_pong() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder.encode(&ClientOp::Pong).to_bytes(),
            "PONG\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_subscribe() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Subscribe {
                    id: 1.into(),
                    subject: Subject::from_static("hello.world"),
                    queue_group: None,
                })
                .to_bytes(),
            "SUB hello.world 1\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_subscribe_with_queue_group() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Subscribe {
                    id: 1.into(),
                    subject: Subject::from_static("hello.world"),
                    queue_group: Some(QueueGroup::from_static("stuff")),
                })
                .to_bytes(),
            "SUB hello.world stuff 1\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_unsubscribe() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Unsubscribe {
                    id: 1.into(),
                    max_messages: None,
                })
                .to_bytes(),
            "UNSUB 1\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_unsubscribe_with_max_messages() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Unsubscribe {
                    id: 1.into(),
                    max_messages: Some(NonZero::new(5).unwrap()),
                })
                .to_bytes(),
            "UNSUB 1 5\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_publish() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Publish {
                    message: MessageBase {
                        subject: Subject::from_static("hello.world"),
                        reply_subject: None,
                        headers: HeaderMap::new(),
                        payload: Bytes::from_static(b"Hello World!"),
                    },
                })
                .to_bytes(),
            "PUB hello.world 12\r\nHello World!\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_publish_with_reply_subject() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder
                .encode(&ClientOp::Publish {
                    message: MessageBase {
                        subject: Subject::from_static("hello.world"),
                        reply_subject: Some(Subject::from_static("_INBOX.1234")),
                        headers: HeaderMap::new(),
                        payload: Bytes::from_static(b"Hello World!"),
                    },
                })
                .to_bytes(),
            "PUB hello.world _INBOX.1234 12\r\nHello World!\r\n".as_bytes()
        );
    }

    #[test]
    fn encode_publish_with_headers() {
        let mut encoder = FramedEncoder::new();
        assert_eq!(
            encoder.encode(&ClientOp::Publish {
                message: MessageBase {
                    subject: Subject::from_static("hello.world"),
                    reply_subject: None,
                    headers: [
                        (
                            HeaderName::from_static("Nats-Message-Id"),
                            HeaderValue::from_static("abcd"),
                        ),
                        (
                            HeaderName::from_static("Nats-Sequence"),
                            HeaderValue::from_static("1"),
                        ),
                    ]
                    .into_iter()
                    .collect(),
                    payload: Bytes::from_static(b"Hello World!"),
                },
            }).to_bytes(),
            "HPUB hello.world 53 65\r\nNATS/1.0\r\nNats-Message-Id: abcd\r\nNats-Sequence: 1\r\n\r\nHello World!\r\n".as_bytes()
        );
    }
}
