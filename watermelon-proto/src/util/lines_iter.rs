use core::{iter, mem};

use bytes::{Buf, Bytes};

pub(crate) fn lines_iter(mut bytes: Bytes) -> impl Iterator<Item = Bytes> {
    iter::from_fn(move || {
        if bytes.is_empty() {
            return None;
        }

        Some(match memchr::memmem::find(&bytes, b"\r\n") {
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
        assert!(expected_chunks.eq(lines_iter(combined_chunk)));
    }
}
