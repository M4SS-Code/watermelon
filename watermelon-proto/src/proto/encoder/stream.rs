#[cfg(feature = "std")]
use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};

use crate::util::BufList;

use super::{ClientOp, FrameEncoder};

const WRITE_FLATTEN_THRESHOLD: usize = 4096;

#[derive(Debug)]
pub struct StreamEncoder {
    write_buf: BufList<Bytes>,
    flattened_writes: BytesMut,
}

impl StreamEncoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            write_buf: BufList::new(),
            flattened_writes: BytesMut::new(),
        }
    }

    pub fn enqueue_write_op(&mut self, item: &ClientOp) {
        super::encode(self, item);
    }

    #[cfg(test)]
    fn all_bytes(&mut self) -> alloc::vec::Vec<u8> {
        self.copy_to_bytes(self.remaining()).to_vec()
    }
}

impl Buf for StreamEncoder {
    fn remaining(&self) -> usize {
        self.write_buf.remaining() + self.flattened_writes.remaining()
    }

    fn has_remaining(&self) -> bool {
        self.write_buf.has_remaining() || self.flattened_writes.has_remaining()
    }

    fn chunk(&self) -> &[u8] {
        let chunk = self.write_buf.chunk();
        if chunk.is_empty() {
            &self.flattened_writes
        } else {
            chunk
        }
    }

    #[cfg(feature = "std")]
    fn chunks_vectored<'a>(&'a self, dst: &mut [io::IoSlice<'a>]) -> usize {
        let mut n = self.write_buf.chunks_vectored(dst);
        n += self.flattened_writes.chunks_vectored(&mut dst[n..]);
        n
    }

    fn advance(&mut self, cnt: usize) {
        assert!(cnt <= self.remaining());

        let mid = self.write_buf.remaining().min(cnt);
        self.write_buf.advance(mid);

        let rem = cnt - mid;
        if rem == self.flattened_writes.len() {
            // https://github.com/tokio-rs/bytes/pull/698
            self.flattened_writes.clear();
        } else {
            self.flattened_writes.advance(rem);
        }
    }

    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        assert!(
            len <= self.remaining(),
            "copy_to_bytes out of range ({} <= {})",
            len,
            self.remaining()
        );

        if self.write_buf.remaining() >= len {
            self.write_buf.copy_to_bytes(len)
        } else if !self.write_buf.has_remaining() {
            self.flattened_writes.copy_to_bytes(len)
        } else {
            let rem = len - self.write_buf.remaining();

            let mut bufs = BytesMut::with_capacity(len);
            bufs.put(&mut self.write_buf);
            bufs.put_slice(&self.flattened_writes[..rem]);

            if self.flattened_writes.remaining() == rem {
                // https://github.com/tokio-rs/bytes/pull/698
                self.flattened_writes.clear();
            } else {
                self.flattened_writes.advance(rem);
            }

            bufs.freeze()
        }
    }
}

impl FrameEncoder for StreamEncoder {
    fn small_write(&mut self, buf: &[u8]) {
        self.flattened_writes.extend_from_slice(buf);
    }

    fn write<B>(&mut self, buf: B)
    where
        B: Into<Bytes> + AsRef<[u8]>,
    {
        let b = buf.as_ref();

        let len = b.len();
        if len == 0 {
            return;
        }

        if len < WRITE_FLATTEN_THRESHOLD {
            self.flattened_writes.extend_from_slice(b);
        } else {
            if !self.flattened_writes.is_empty() {
                let buf = self.flattened_writes.split().freeze();
                self.write_buf.push(buf);
            }

            self.write_buf.push(buf.into());
        }
    }
}

impl Default for StreamEncoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use core::num::NonZeroU64;
    #[cfg(feature = "std")]
    use std::io::IoSlice;

    use bytes::{Buf as _, Bytes};

    use super::StreamEncoder;
    #[cfg(feature = "std")]
    use crate::proto::encoder::FrameEncoder;
    use crate::{
        MessageBase, QueueGroup, Subject,
        headers::{HeaderMap, HeaderName, HeaderValue},
        proto::ClientOp,
    };

    #[test]
    fn base() {
        let encoder = StreamEncoder::new();
        assert_eq!(0, encoder.remaining());
        assert!(!encoder.has_remaining());
    }

    #[cfg(feature = "std")]
    #[test]
    fn vectored() {
        let mut encoder = StreamEncoder::new();
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(0, encoder.chunks_vectored(&mut bufs));

        encoder.small_write(b"1234");
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(1, encoder.chunks_vectored(&mut bufs));

        encoder.small_write(b"5678");
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(1, encoder.chunks_vectored(&mut bufs));

        encoder.write("9");
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(1, encoder.chunks_vectored(&mut bufs));

        encoder.write(vec![b'_'; 8196]);
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(2, encoder.chunks_vectored(&mut bufs));

        encoder.write(vec![b':'; 8196]);
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(3, encoder.chunks_vectored(&mut bufs));

        encoder.write("0");
        let mut bufs = [IoSlice::new(&[]); 64];
        assert_eq!(4, encoder.chunks_vectored(&mut bufs));
    }

    #[test]
    fn encode_ping() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Ping);
        assert_eq!(6, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!("PING\r\n".as_bytes(), encoder.all_bytes());
    }

    #[test]
    fn encode_pong() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Pong);
        assert_eq!(6, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!("PONG\r\n".as_bytes(), encoder.all_bytes());
    }

    #[test]
    fn encode_subscribe() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Subscribe {
            id: 1.into(),
            subject: Subject::from_static("hello.world"),
            queue_group: None,
        });
        assert_eq!(19, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!("SUB hello.world 1\r\n".as_bytes(), encoder.all_bytes());
    }

    #[test]
    fn encode_subscribe_with_queue_group() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Subscribe {
            id: 1.into(),
            subject: Subject::from_static("hello.world"),
            queue_group: Some(QueueGroup::from_static("stuff")),
        });
        assert_eq!(25, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!(
            "SUB hello.world stuff 1\r\n".as_bytes(),
            encoder.all_bytes()
        );
    }

    #[test]
    fn encode_unsubscribe() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Unsubscribe {
            id: 1.into(),
            max_messages: None,
        });
        assert_eq!(9, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!("UNSUB 1\r\n".as_bytes(), encoder.all_bytes());
    }

    #[test]
    fn encode_unsubscribe_with_max_messages() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Unsubscribe {
            id: 1.into(),
            max_messages: Some(NonZeroU64::new(5).unwrap()),
        });
        assert_eq!(11, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!("UNSUB 1 5\r\n".as_bytes(), encoder.all_bytes());
    }

    #[test]
    fn encode_publish() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Publish {
            message: MessageBase {
                subject: Subject::from_static("hello.world"),
                reply_subject: None,
                headers: HeaderMap::new(),
                payload: Bytes::from_static(b"Hello World!"),
            },
        });
        assert_eq!(34, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!(
            "PUB hello.world 12\r\nHello World!\r\n".as_bytes(),
            encoder.all_bytes()
        );
    }

    #[test]
    fn encode_publish_with_reply_subject() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Publish {
            message: MessageBase {
                subject: Subject::from_static("hello.world"),
                reply_subject: Some(Subject::from_static("_INBOX.1234")),
                headers: HeaderMap::new(),
                payload: Bytes::from_static(b"Hello World!"),
            },
        });
        assert_eq!(46, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!(
            "PUB hello.world _INBOX.1234 12\r\nHello World!\r\n".as_bytes(),
            encoder.all_bytes()
        );
    }

    #[test]
    fn encode_publish_with_headers() {
        let mut encoder = StreamEncoder::new();
        encoder.enqueue_write_op(&ClientOp::Publish {
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
        });
        assert_eq!(91, encoder.remaining());
        assert!(encoder.has_remaining());
        assert_eq!(
            "HPUB hello.world 53 65\r\nNATS/1.0\r\nNats-Message-Id: abcd\r\nNats-Sequence: 1\r\n\r\nHello World!\r\n".as_bytes(),
            encoder.all_bytes()
        );
    }
}
