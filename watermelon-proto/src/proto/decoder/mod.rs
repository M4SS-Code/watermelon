use core::{mem, ops::Deref};

use bytes::{Buf, Bytes, BytesMut};
use bytestring::ByteString;

use crate::{
    error::ServerError,
    headers::{
        error::{HeaderNameValidateError, HeaderValueValidateError},
        HeaderMap, HeaderName, HeaderValue,
    },
    status_code::StatusCodeError,
    util::{self, ParseUintError},
    MessageBase, ServerMessage, StatusCode, Subject, SubscriptionId,
};

pub use self::framed::{decode_frame, FrameDecoderError};
pub use self::stream::StreamDecoder;

use super::ServerOp;

mod framed;
mod stream;

const MAX_HEAD_LEN: usize = 16 * 1024;

#[derive(Debug)]
pub(super) enum DecoderStatus {
    ControlLine {
        last_bytes_read: usize,
    },
    Headers {
        subscription_id: SubscriptionId,
        subject: Subject,
        reply_subject: Option<Subject>,
        header_len: usize,
        payload_len: usize,
    },
    Payload {
        subscription_id: SubscriptionId,
        subject: Subject,
        reply_subject: Option<Subject>,
        status_code: Option<StatusCode>,
        headers: HeaderMap,
        payload_len: usize,
    },
    Poisoned,
}

pub(super) trait BytesLike: Buf + Deref<Target = [u8]> {
    fn len(&self) -> usize {
        Buf::remaining(self)
    }

    fn split_to(&mut self, at: usize) -> Bytes {
        self.copy_to_bytes(at)
    }
}

impl BytesLike for Bytes {}
impl BytesLike for BytesMut {}

pub(super) fn decode(
    status: &mut DecoderStatus,
    read_buf: &mut impl BytesLike,
) -> Result<Option<ServerOp>, DecoderError> {
    loop {
        match status {
            DecoderStatus::ControlLine { last_bytes_read } => {
                if read_buf.starts_with(b"+OK\r\n") {
                    // Fast path for handling `+OK`
                    debug_assert_eq!(
                        *last_bytes_read, 0,
                        "we shouldn't have handled any bytes before"
                    );
                    read_buf.advance("+OK\r\n".len());
                    return Ok(Some(ServerOp::Success));
                }

                if *last_bytes_read == read_buf.len() {
                    // No progress has been made
                    return Ok(None);
                }

                let Some(control_line_len) = memchr::memmem::find(read_buf, b"\r\n") else {
                    return if read_buf.len() < MAX_HEAD_LEN {
                        *last_bytes_read = read_buf.len();
                        Ok(None)
                    } else {
                        Err(DecoderError::HeadTooLong {
                            len: read_buf.len(),
                        })
                    };
                };

                let mut control_line = read_buf.split_to(control_line_len + "\r\n".len());
                control_line.truncate(control_line.len() - 2);

                return if control_line.starts_with(b"MSG ") {
                    *status = decode_msg(control_line)?;
                    continue;
                } else if control_line.starts_with(b"HMSG ") {
                    *status = decode_hmsg(control_line)?;
                    continue;
                } else if control_line.starts_with(b"PING") {
                    Ok(Some(ServerOp::Ping))
                } else if control_line.starts_with(b"PONG") {
                    Ok(Some(ServerOp::Pong))
                } else if control_line.starts_with(b"+OK") {
                    // Slow path for handling `+OK`
                    Ok(Some(ServerOp::Success))
                } else if control_line.starts_with(b"-ERR ") {
                    control_line.advance("-ERR ".len());
                    if !control_line.starts_with(b"'") || !control_line.ends_with(b"'") {
                        return Err(DecoderError::InvalidErrorMessage);
                    }

                    control_line.advance(1);
                    control_line.truncate(control_line.len() - 1);
                    let raw_message = ByteString::try_from(control_line)
                        .map_err(|_| DecoderError::InvalidErrorMessage)?;
                    let error = ServerError::parse(raw_message);
                    Ok(Some(ServerOp::Error { error }))
                } else if let Some(info) = control_line.strip_prefix(b"INFO ") {
                    let info = serde_json::from_slice(info).map_err(DecoderError::InvalidInfo)?;
                    Ok(Some(ServerOp::Info { info }))
                } else {
                    Err(DecoderError::InvalidCommand)
                };
            }
            DecoderStatus::Headers { header_len, .. } => {
                if read_buf.len() < *header_len {
                    return Ok(None);
                }

                decode_headers(read_buf, status)?;
            }
            DecoderStatus::Payload { payload_len, .. } => {
                if read_buf.len() < *payload_len + "\r\n".len() {
                    return Ok(None);
                }

                let DecoderStatus::Payload {
                    subscription_id,
                    subject,
                    reply_subject,
                    status_code,
                    headers,
                    payload_len,
                } = mem::replace(status, DecoderStatus::ControlLine { last_bytes_read: 0 })
                else {
                    unreachable!()
                };

                let payload = read_buf.split_to(payload_len);
                read_buf.advance("\r\n".len());
                let message = ServerMessage {
                    status_code,
                    subscription_id,
                    base: MessageBase {
                        subject,
                        reply_subject,
                        headers,
                        payload,
                    },
                };
                return Ok(Some(ServerOp::Message { message }));
            }
            DecoderStatus::Poisoned => return Err(DecoderError::Poisoned),
        }
    }
}

fn decode_msg(mut control_line: Bytes) -> Result<DecoderStatus, DecoderError> {
    control_line.advance("MSG ".len());

    let mut chunks = util::split_spaces(control_line);
    let (subject, subscription_id, reply_subject, payload_len) = match (
        chunks.next(),
        chunks.next(),
        chunks.next(),
        chunks.next(),
        chunks.next(),
    ) {
        (Some(subject), Some(subscription_id), Some(reply_subject), Some(payload_len), None) => {
            (subject, subscription_id, Some(reply_subject), payload_len)
        }
        (Some(subject), Some(subscription_id), Some(payload_len), None, None) => {
            (subject, subscription_id, None, payload_len)
        }
        _ => return Err(DecoderError::InvalidMsgArgsCount),
    };
    let subject = Subject::from_dangerous_value(
        subject
            .try_into()
            .map_err(|_| DecoderError::SubjectInvalidUtf8)?,
    );
    let subscription_id =
        SubscriptionId::from_ascii_bytes(&subscription_id).map_err(DecoderError::SubscriptionId)?;
    let reply_subject = reply_subject
        .map(|reply_subject| {
            ByteString::try_from(reply_subject).map_err(|_| DecoderError::SubjectInvalidUtf8)
        })
        .transpose()?
        .map(Subject::from_dangerous_value);
    let payload_len =
        util::parse_usize(&payload_len).map_err(DecoderError::InvalidPayloadLength)?;
    Ok(DecoderStatus::Payload {
        subscription_id,
        subject,
        reply_subject,
        status_code: None,
        headers: HeaderMap::new(),
        payload_len,
    })
}

fn decode_hmsg(mut control_line: Bytes) -> Result<DecoderStatus, DecoderError> {
    control_line.advance("HMSG ".len());
    let mut chunks = util::split_spaces(control_line);

    let (subject, subscription_id, reply_subject, header_len, total_len) = match (
        chunks.next(),
        chunks.next(),
        chunks.next(),
        chunks.next(),
        chunks.next(),
        chunks.next(),
    ) {
        (
            Some(subject),
            Some(subscription_id),
            Some(reply_to),
            Some(header_len),
            Some(total_len),
            None,
        ) => (
            subject,
            subscription_id,
            Some(reply_to),
            header_len,
            total_len,
        ),
        (Some(subject), Some(subscription_id), Some(header_len), Some(total_len), None, None) => {
            (subject, subscription_id, None, header_len, total_len)
        }
        _ => return Err(DecoderError::InvalidHmsgArgsCount),
    };

    let subject = Subject::from_dangerous_value(
        subject
            .try_into()
            .map_err(|_| DecoderError::SubjectInvalidUtf8)?,
    );
    let subscription_id =
        SubscriptionId::from_ascii_bytes(&subscription_id).map_err(DecoderError::SubscriptionId)?;
    let reply_subject = reply_subject
        .map(|reply_subject| {
            ByteString::try_from(reply_subject).map_err(|_| DecoderError::SubjectInvalidUtf8)
        })
        .transpose()?
        .map(Subject::from_dangerous_value);
    let header_len = util::parse_usize(&header_len).map_err(DecoderError::InvalidHeaderLength)?;
    let total_len = util::parse_usize(&total_len).map_err(DecoderError::InvalidPayloadLength)?;

    let payload_len = total_len
        .checked_sub(header_len)
        .ok_or(DecoderError::InvalidTotalLength)?;

    Ok(DecoderStatus::Headers {
        subscription_id,
        subject,
        reply_subject,
        header_len,
        payload_len,
    })
}

fn decode_headers(
    read_buf: &mut impl BytesLike,
    status: &mut DecoderStatus,
) -> Result<(), DecoderError> {
    let DecoderStatus::Headers {
        subscription_id,
        subject,
        reply_subject,
        header_len,
        payload_len,
    } = mem::replace(status, DecoderStatus::Poisoned)
    else {
        unreachable!()
    };

    let header = read_buf.split_to(header_len);
    let mut lines = util::lines_iter(header);
    let head = lines.next().ok_or(DecoderError::MissingHead)?;
    let head = head
        .strip_prefix(b"NATS/1.0")
        .ok_or(DecoderError::InvalidHead)?;
    let status_code = if head.len() >= 4 {
        Some(StatusCode::from_ascii_bytes(&head[1..4]).map_err(DecoderError::StatusCode)?)
    } else {
        None
    };

    let headers = lines
        .filter(|line| !line.is_empty())
        .map(|mut line| {
            let i = memchr::memchr(b':', &line).ok_or(DecoderError::InvalidHeaderLine)?;

            let name = line.split_to(i);
            line.advance(":".len());
            if line[0].is_ascii_whitespace() {
                // The fact that this is allowed sounds like BS to me
                line.advance(1);
            }
            let value = line;

            let name = HeaderName::try_from(
                ByteString::try_from(name).map_err(|_| DecoderError::HeaderNameInvalidUtf8)?,
            )
            .map_err(DecoderError::HeaderName)?;
            let value = HeaderValue::try_from(
                ByteString::try_from(value).map_err(|_| DecoderError::HeaderValueInvalidUtf8)?,
            )
            .map_err(DecoderError::HeaderValue)?;
            Ok((name, value))
        })
        .collect::<Result<_, _>>()?;

    *status = DecoderStatus::Payload {
        subscription_id,
        subject,
        reply_subject,
        status_code,
        headers,
        payload_len,
    };
    Ok(())
}

#[derive(Debug, thiserror::Error)]
pub enum DecoderError {
    #[error("The head exceeded the maximum head length (len {len} maximum {MAX_HEAD_LEN}")]
    HeadTooLong { len: usize },
    #[error("Invalid command")]
    InvalidCommand,
    #[error("MSG command has an unexpected number of arguments")]
    InvalidMsgArgsCount,
    #[error("HMSG command has an unexpected number of arguments")]
    InvalidHmsgArgsCount,
    #[error("The subject isn't valid utf-8")]
    SubjectInvalidUtf8,
    #[error("The reply subject isn't valid utf-8")]
    ReplySubjectInvalidUtf8,
    #[error("Couldn't parse the Subscription ID")]
    SubscriptionId(#[source] ParseUintError),
    #[error("Couldn't parse the length of the header")]
    InvalidHeaderLength(#[source] ParseUintError),
    #[error("Couldn't parse the length of the payload")]
    InvalidPayloadLength(#[source] ParseUintError),
    #[error("The total length is greater than the header length")]
    InvalidTotalLength,
    #[error("HMSG is missing head")]
    MissingHead,
    #[error("HMSG has an invalid head")]
    InvalidHead,
    #[error("HMSG header line is missing ': '")]
    InvalidHeaderLine,
    #[error("Couldn't parse the status code")]
    StatusCode(#[source] StatusCodeError),
    #[error("The header name isn't valid utf-8")]
    HeaderNameInvalidUtf8,
    #[error("The header name coouldn't be parsed")]
    HeaderName(#[source] HeaderNameValidateError),
    #[error("The header value isn't valid utf-8")]
    HeaderValueInvalidUtf8,
    #[error("The header value coouldn't be parsed")]
    HeaderValue(#[source] HeaderValueValidateError),
    #[error("INFO command JSON payload couldn't be deserialized")]
    InvalidInfo(#[source] serde_json::Error),
    #[error("-ERR command message couldn't be deserialized")]
    InvalidErrorMessage,
    #[error("The decoder was poisoned")]
    Poisoned,
}
