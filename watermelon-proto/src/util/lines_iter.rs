use core::{iter, mem};

use bytes::{Buf, Bytes};

use super::CrlfFinder;

pub(crate) fn lines_iter(crlf: &CrlfFinder, mut bytes: Bytes) -> impl Iterator<Item = Bytes> + '_ {
    iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }

        Some(match crlf.find(&bytes) {
            Some(i) => {
                let chunk = bytes.split_to(i);
                bytes.advance("\r\n".len());
                chunk
            }
            None => mem::take(&mut bytes),
        })
    })
}

#[cfg(test)]
mod tests {
    use bytes::{Bytes, BytesMut};

    use crate::util::CrlfFinder;

    use super::lines_iter;

    #[test]
    fn iterate_lines() {
        let expected_chunks = ["", "abcd", "12334534", "alkfdasfsd", "", "-"];
        let mut combined_chunk = expected_chunks
            .iter()
            .fold(BytesMut::new(), |mut buf, chunk| {
                buf.extend_from_slice(chunk.as_bytes());
                buf.extend_from_slice(b"\r\n");
                buf
            });
        combined_chunk.truncate(combined_chunk.len() - "\r\n".len());
        let combined_chunk = combined_chunk.freeze();

        let expected_chunks = expected_chunks
            .iter()
            .map(|c| Bytes::from_static(c.as_bytes()));
        assert!(expected_chunks.eq(lines_iter(&CrlfFinder::new(), combined_chunk)));
    }
}
