use bytes::Bytes;

use crate::{proto::ServerOp, util::CrlfFinder};

use super::{DecoderError, DecoderStatus};

/// Decodes a frame of bytes into a [`ServerOp`].
///
/// # Errors
///
/// It returns an error in case the frame is incomplete or if a decoding error occurs.
pub fn decode_frame(frame: &mut Bytes) -> Result<ServerOp, FrameDecoderError> {
    let mut status = DecoderStatus::ControlLine { last_bytes_read: 0 };
    match super::decode(&CrlfFinder::new(), &mut status, frame) {
        Ok(Some(server_op)) => Ok(server_op),
        Ok(None) => Err(FrameDecoderError::IncompleteFrame),
        Err(err) => Err(FrameDecoderError::Decoder(err)),
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FrameDecoderError {
    #[error("incomplete frame")]
    IncompleteFrame,
    #[error("decoder error")]
    Decoder(#[source] DecoderError),
}
