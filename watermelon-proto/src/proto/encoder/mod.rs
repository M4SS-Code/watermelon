#[cfg(feature = "std")]
use std::io;

use bytes::Bytes;

use crate::MessageBase;
use crate::headers::HeaderMap;

pub use self::framed::FramedEncoder;
pub use self::stream::StreamEncoder;

use super::ClientOp;

mod framed;
mod stream;

pub(super) trait FrameEncoder {
    fn small_write(&mut self, buf: &[u8]);

    fn write<B>(&mut self, buf: B)
    where
        B: Into<Bytes> + AsRef<[u8]>,
    {
        self.small_write(buf.as_ref());
    }

    #[cfg(feature = "std")]
    fn small_io_writer(&mut self) -> SmallIoWriter<'_, Self> {
        SmallIoWriter(self)
    }
}

#[cfg(feature = "std")]
pub(super) struct SmallIoWriter<'a, E: ?Sized>(&'a mut E);

#[cfg(feature = "std")]
impl<E> io::Write for SmallIoWriter<'_, E>
where
    E: FrameEncoder,
{
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        self.0.small_write(buf);
        Ok(buf.len())
    }

    fn write_all(&mut self, buf: &[u8]) -> io::Result<()> {
        self.0.small_write(buf);
        Ok(())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

pub(super) fn encode<E: FrameEncoder>(encoder: &mut E, item: &ClientOp) {
    match item {
        ClientOp::Publish { message } => {
            let MessageBase {
                subject,
                reply_subject,
                headers,
                payload,
            } = &message;
            let verb_and_space = if headers.is_empty() { "PUB " } else { "HPUB " };
            encoder.small_write(verb_and_space.as_bytes());
            encoder.small_write(subject.as_bytes());
            encoder.small_write(b" ");

            if let Some(reply_subject) = reply_subject {
                encoder.small_write(reply_subject.as_bytes());
                encoder.small_write(b" ");
            }

            let mut buffer = itoa::Buffer::new();
            if headers.is_empty() {
                encoder.small_write(buffer.format(payload.len()).as_bytes());
                encoder.small_write(b"\r\n");
            } else {
                let headers_len = encode_headers(headers).fold(0, |len, s| len + s.len());
                encoder.small_write(buffer.format(headers_len).as_bytes());
                encoder.small_write(b" ");

                let total_len = headers_len + payload.len();
                encoder.small_write(buffer.format(total_len).as_bytes());
                encoder.small_write(b"\r\n");

                encode_headers(headers).for_each(|s| {
                    encoder.small_write(s.as_bytes());
                });
            }

            encoder.write(IntoBytes(payload));
            encoder.small_write(b"\r\n");
        }
        ClientOp::Subscribe {
            id,
            subject,
            queue_group,
        } => {
            // `SUB {subject} [{queue_group} ]id\r\n`
            encoder.small_write(b"SUB ");
            encoder.small_write(subject.as_bytes());
            encoder.small_write(b" ");

            if let Some(queue_group) = queue_group {
                encoder.small_write(queue_group.as_bytes());
                encoder.small_write(b" ");
            }

            let mut buffer = itoa::Buffer::new();
            encoder.small_write(buffer.format(u64::from(*id)).as_bytes());
            encoder.small_write(b"\r\n");
        }
        ClientOp::Unsubscribe { id, max_messages } => {
            // `UNSUB {id}[ {max_messages}]\r\n`
            encoder.small_write(b"UNSUB ");

            let mut buffer = itoa::Buffer::new();
            encoder.small_write(buffer.format(u64::from(*id)).as_bytes());

            if let Some(max_messages) = *max_messages {
                encoder.small_write(b" ");
                encoder.small_write(buffer.format(max_messages.get()).as_bytes());
            }

            encoder.small_write(b"\r\n");
        }
        ClientOp::Connect { connect } => {
            encoder.small_write(b"CONNECT ");
            #[cfg(feature = "std")]
            serde_json::to_writer(encoder.small_io_writer(), &connect)
                .expect("serialize `Connect`");
            #[cfg(not(feature = "std"))]
            encoder.write(serde_json::to_vec(&connect).expect("serialize `Connect`"));
            encoder.small_write(b"\r\n");
        }
        ClientOp::Ping => {
            encoder.small_write(b"PING\r\n");
        }
        ClientOp::Pong => {
            encoder.small_write(b"PONG\r\n");
        }
    }
}

struct IntoBytes<'a>(&'a Bytes);

impl<'a> From<IntoBytes<'a>> for Bytes {
    fn from(value: IntoBytes<'a>) -> Self {
        Bytes::clone(value.0)
    }
}

impl AsRef<[u8]> for IntoBytes<'_> {
    fn as_ref(&self) -> &[u8] {
        self.0
    }
}

fn encode_headers(headers: &HeaderMap) -> impl Iterator<Item = &'_ str> {
    let head = ["NATS/1.0\r\n"];
    let headers = headers.iter().flat_map(|(name, values)| {
        values.flat_map(|value| [name.as_str(), ": ", value.as_str(), "\r\n"])
    });
    let footer = ["\r\n"];

    head.into_iter().chain(headers).chain(footer)
}
