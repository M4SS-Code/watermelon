use bytes::{BufMut, BytesMut};

use crate::{
    proto::{ServerOp, error::DecoderError},
    util::CrlfFinder,
};

use super::DecoderStatus;

const INITIAL_READ_BUF_CAPACITY: usize = 64 * 1024;

#[derive(Debug)]
pub struct StreamDecoder {
    read_buf: BytesMut,
    status: DecoderStatus,
    crlf: CrlfFinder,
}

impl StreamDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            read_buf: BytesMut::with_capacity(INITIAL_READ_BUF_CAPACITY),
            status: DecoderStatus::ControlLine { last_bytes_read: 0 },
            crlf: CrlfFinder::new(),
        }
    }

    #[must_use]
    pub fn read_buf(&mut self) -> &mut (impl BufMut + use<>) {
        &mut self.read_buf
    }

    /// Decodes the next frame of bytes into a [`ServerOp`].
    ///
    /// A `None` variant is returned in case no progress is made,
    ///
    /// # Errors
    ///
    /// It returns an error if a decoding error occurs.
    pub fn decode(&mut self) -> Result<Option<ServerOp>, DecoderError> {
        super::decode(&self.crlf, &mut self.status, &mut self.read_buf)
    }
}

impl Default for StreamDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use bytes::{BufMut as _, Bytes};
    use claims::{assert_matches, assert_ok_eq};

    use bytestring::ByteString;

    use crate::{
        StatusCode, Subject,
        error::ServerError,
        headers::{HeaderMap, HeaderName, HeaderValue},
        message::{MessageBase, ServerMessage},
        proto::{error::DecoderError, server::ServerOp},
    };

    use super::StreamDecoder;

    #[test]
    fn decode_ping() {
        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(b"PING\r\n");
        assert_ok_eq!(decoder.decode(), Some(ServerOp::Ping));
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_pong() {
        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(b"PONG\r\n");
        assert_ok_eq!(decoder.decode(), Some(ServerOp::Pong));
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_ok() {
        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(b"+OK\r\n");
        assert_ok_eq!(decoder.decode(), Some(ServerOp::Success));
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_error() {
        let mut decoder = StreamDecoder::new();
        decoder
            .read_buf()
            .put_slice(b"-ERR 'Authorization Violation'\r\n");
        assert_ok_eq!(
            decoder.decode(),
            Some(ServerOp::Error {
                error: ServerError::AuthorizationViolation
            })
        );
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_msg() {
        let mut decoder = StreamDecoder::new();
        decoder
            .read_buf()
            .put_slice(b"MSG hello.world 1 12\r\nHello World!\r\n");
        assert_ok_eq!(
            decoder.decode(),
            Some(ServerOp::Message {
                message: ServerMessage {
                    status_code: None,
                    status_description: None,
                    subscription_id: 1.into(),
                    base: MessageBase {
                        subject: Subject::from_static("hello.world"),
                        reply_subject: None,
                        headers: HeaderMap::new(),
                        payload: Bytes::from_static(b"Hello World!")
                    }
                }
            })
        );
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_hmsg_status_description() {
        const HEADERS: &str = "NATS/1.0 409 Batch Completed\r\nNats-Pending-Messages: 3\r\nNats-Pending-Bytes: 0\r\n\r\n";

        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(
            format!(
                "HMSG _INBOX.abcd 9 {} {}\r\n{HEADERS}\r\n",
                HEADERS.len(),
                HEADERS.len()
            )
            .as_bytes(),
        );
        assert_ok_eq!(
            decoder.decode(),
            Some(ServerOp::Message {
                message: ServerMessage {
                    status_code: Some(StatusCode::CONFLICT),
                    status_description: Some(ByteString::from_static("Batch Completed")),
                    subscription_id: 9.into(),
                    base: MessageBase {
                        subject: Subject::from_static("_INBOX.abcd"),
                        reply_subject: None,
                        headers: HeaderMap::from_iter([
                            (
                                HeaderName::from_static("Nats-Pending-Messages"),
                                HeaderValue::from_static("3")
                            ),
                            (
                                HeaderName::from_static("Nats-Pending-Bytes"),
                                HeaderValue::from_static("0")
                            ),
                        ]),
                        payload: Bytes::new(),
                    }
                }
            })
        );
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_hmsg_status_without_description() {
        const HEADERS: &str = "NATS/1.0 100\r\n\r\n";

        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(
            format!(
                "HMSG _INBOX.abcd 9 {} {}\r\n{HEADERS}\r\n",
                HEADERS.len(),
                HEADERS.len()
            )
            .as_bytes(),
        );
        assert_ok_eq!(
            decoder.decode(),
            Some(ServerOp::Message {
                message: ServerMessage {
                    status_code: Some(StatusCode::IDLE_HEARTBEAT),
                    status_description: None,
                    subscription_id: 9.into(),
                    base: MessageBase {
                        subject: Subject::from_static("_INBOX.abcd"),
                        reply_subject: None,
                        headers: HeaderMap::new(),
                        payload: Bytes::new(),
                    }
                }
            })
        );
        assert_ok_eq!(decoder.decode(), None);
    }

    #[test]
    fn decode_hmsg_status_invalid_description_separator() {
        const HEADERS: &str = "NATS/1.0 409XBatch Completed\r\n\r\n";

        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_slice(
            format!(
                "HMSG _INBOX.abcd 9 {} {}\r\n{HEADERS}\r\n",
                HEADERS.len(),
                HEADERS.len()
            )
            .as_bytes(),
        );
        assert_matches!(decoder.decode(), Err(DecoderError::InvalidHead));
    }

    #[test]
    fn head_too_long() {
        let mut decoder = StreamDecoder::new();
        decoder.read_buf().put_bytes(0, 20000);
        assert_matches!(
            decoder.decode(),
            Err(DecoderError::HeadTooLong { len: 20000 })
        );
    }
}
