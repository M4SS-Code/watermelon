use bytes::{BufMut, BytesMut};

use crate::proto::{error::DecoderError, ServerOp};

use super::DecoderStatus;

const INITIAL_READ_BUF_CAPACITY: usize = 64 * 1024;

#[derive(Debug)]
pub struct StreamDecoder {
    read_buf: BytesMut,
    status: DecoderStatus,
}

impl StreamDecoder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            read_buf: BytesMut::with_capacity(INITIAL_READ_BUF_CAPACITY),
            status: DecoderStatus::ControlLine { last_bytes_read: 0 },
        }
    }

    #[must_use]
    pub fn read_buf(&mut self) -> &mut impl BufMut {
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
        super::decode(&mut self.status, &mut self.read_buf)
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
    use claims::assert_ok_eq;

    use crate::{
        error::ServerError,
        headers::HeaderMap,
        message::{MessageBase, ServerMessage},
        proto::server::ServerOp,
        Subject,
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
}
