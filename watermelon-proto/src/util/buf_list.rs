use alloc::collections::VecDeque;
use core::cmp::Ordering;
#[cfg(feature = "std")]
use std::io;

use bytes::{Buf, BufMut, Bytes, BytesMut};

#[derive(Debug)]
pub(crate) struct BufList<B> {
    bufs: VecDeque<B>,
    len: usize,
}

impl<B: Buf> BufList<B> {
    pub(crate) const fn new() -> Self {
        Self {
            bufs: VecDeque::new(),
            len: 0,
        }
    }

    pub(crate) fn push(&mut self, buf: B) {
        debug_assert!(buf.has_remaining());
        let rem = buf.remaining();
        self.bufs.push_back(buf);
        self.len += rem;
    }
}

impl<B: Buf> Buf for BufList<B> {
    fn remaining(&self) -> usize {
        debug_assert_eq!(
            self.len,
            self.bufs.iter().map(Buf::remaining).sum::<usize>()
        );
        self.len
    }

    fn has_remaining(&self) -> bool {
        !self.bufs.is_empty()
    }

    fn chunk(&self) -> &[u8] {
        self.bufs.front().map(Buf::chunk).unwrap_or_default()
    }

    fn advance(&mut self, mut cnt: usize) {
        assert!(
            cnt <= self.remaining(),
            "advance out of range ({} <= {})",
            cnt,
            self.remaining()
        );

        while cnt > 0 {
            let entry = self.bufs.front_mut().unwrap();
            let remaining = entry.remaining();
            if remaining > cnt {
                entry.advance(cnt);
                self.len -= cnt;
                cnt -= cnt;
            } else {
                let _ = self.bufs.pop_front();
                self.len -= remaining;
                cnt -= remaining;
            }
        }
    }

    #[cfg(feature = "std")]
    fn chunks_vectored<'a>(&'a self, mut dst: &mut [io::IoSlice<'a>]) -> usize {
        let mut filled = 0;
        for buf in &self.bufs {
            let n = buf.chunks_vectored(dst);
            filled += n;

            dst = &mut dst[n..];
            if dst.is_empty() {
                break;
            }
        }

        filled
    }

    fn copy_to_bytes(&mut self, len: usize) -> Bytes {
        assert!(
            len <= self.remaining(),
            "copy_to_bytes out of range ({} <= {})",
            len,
            self.remaining()
        );

        if let Some(first) = self.bufs.front_mut() {
            match first.remaining().cmp(&len) {
                Ordering::Greater => {
                    self.len -= len;
                    return first.copy_to_bytes(len);
                }
                Ordering::Equal => {
                    self.len -= len;
                    return self.bufs.pop_front().unwrap().copy_to_bytes(len);
                }
                Ordering::Less => {}
            }
        }

        let mut bufs = BytesMut::with_capacity(len);
        bufs.put(self.take(len));
        bufs.freeze()
    }
}

#[cfg(test)]
mod tests {
    #[cfg(feature = "std")]
    use std::io;

    use bytes::{buf::Chain, Buf};

    use super::BufList;

    #[test]
    fn series() {
        let mut bufs = BufList::<Chain<&[u8], &[u8]>>::new();
        // empty
        assert_eq!(bufs.remaining(), 0);
        assert!(!bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[0u8; 0]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 0);
            assert!(vectors.iter().all(|vector| vector.is_empty()));
        }

        // first push
        bufs.push([0u8, 1, 2, 3, 4, 5, 6, 7].chain(&[8u8, 9, 10, 11]));
        assert_eq!(bufs.remaining(), 12);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[0u8, 1, 2, 3, 4, 5, 6, 7]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 2);
            assert_eq!(&*vectors[0], &[0u8, 1, 2, 3, 4, 5, 6, 7]);
            assert_eq!(&*vectors[1], &[8u8, 9, 10, 11]);
            assert!(vectors[2..].iter().all(|vector| vector.is_empty()));
        }

        // partially read the first buf
        bufs.advance(4);
        assert_eq!(bufs.remaining(), 8);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[4u8, 5, 6, 7]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 2);
            assert_eq!(&*vectors[0], &[4u8, 5, 6, 7]);
            assert_eq!(&*vectors[1], &[8u8, 9, 10, 11]);
            assert!(vectors[2..].iter().all(|vector| vector.is_empty()));
        }

        // second push
        bufs.push([12u8, 13, 14, 15, 16, 17].chain(&[18u8, 19, 20]));
        assert_eq!(bufs.remaining(), 17);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[4u8, 5, 6, 7]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 4);
            assert_eq!(&*vectors[0], &[4u8, 5, 6, 7]);
            assert_eq!(&*vectors[1], &[8u8, 9, 10, 11]);
            assert_eq!(&*vectors[2], &[12u8, 13, 14, 15, 16, 17]);
            assert_eq!(&*vectors[3], &[18u8, 19, 20]);
            assert!(vectors[4..].iter().all(|vector| vector.is_empty()));
        }

        // read remaining first buf and part of second buf
        bufs.advance(6);
        assert_eq!(bufs.remaining(), 11);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[10u8, 11]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 3);
            assert_eq!(&*vectors[0], &[10u8, 11]);
            assert_eq!(&*vectors[1], &[12u8, 13, 14, 15, 16, 17]);
            assert_eq!(&*vectors[2], &[18u8, 19, 20]);
            assert!(vectors[3..].iter().all(|vector| vector.is_empty()));
        }

        // third push
        bufs.push([21u8, 22, 23, 24].chain(&[25u8, 26, 27]));
        assert_eq!(bufs.remaining(), 18);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[10u8, 11]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 5);
            assert_eq!(&*vectors[0], &[10u8, 11]);
            assert_eq!(&*vectors[1], &[12u8, 13, 14, 15, 16, 17]);
            assert_eq!(&*vectors[2], &[18u8, 19, 20]);
            assert_eq!(&*vectors[3], &[21u8, 22, 23, 24]);
            assert_eq!(&*vectors[4], &[25u8, 26, 27]);
            assert!(vectors[5..].iter().all(|vector| vector.is_empty()));
        }

        // read part of second buf
        let buf = bufs.copy_to_bytes(1);
        assert_eq!(&*buf, &[10u8]);
        assert_eq!(bufs.remaining(), 17);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[11u8]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 5);
            assert_eq!(&*vectors[0], &[11u8]);
            assert_eq!(&*vectors[1], &[12u8, 13, 14, 15, 16, 17]);
            assert_eq!(&*vectors[2], &[18u8, 19, 20]);
            assert_eq!(&*vectors[3], &[21u8, 22, 23, 24]);
            assert_eq!(&*vectors[4], &[25u8, 26, 27]);
            assert!(vectors[5..].iter().all(|vector| vector.is_empty()));
        }

        // read remaining second buf and part of third buf
        let buf = bufs.copy_to_bytes(4);
        assert_eq!(&*buf, &[11u8, 12, 13, 14]);
        assert_eq!(bufs.remaining(), 13);
        assert!(bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[15u8, 16, 17]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 4);
            assert_eq!(&*vectors[0], &[15u8, 16, 17]);
            assert_eq!(&*vectors[1], &[18u8, 19, 20]);
            assert_eq!(&*vectors[2], &[21u8, 22, 23, 24]);
            assert_eq!(&*vectors[3], &[25u8, 26, 27]);
            assert!(vectors[4..].iter().all(|vector| vector.is_empty()));
        }

        // read remaining
        let buf = bufs.copy_to_bytes(13);
        assert_eq!(
            &*buf,
            &[15u8, 16, 17, 18, 19, 20, 21, 22, 23, 24, 25, 26, 27]
        );
        assert_eq!(bufs.remaining(), 0);
        assert!(!bufs.has_remaining());
        assert_eq!(bufs.chunk(), &[0u8; 0]);
        #[cfg(feature = "std")]
        {
            let mut vectors = [io::IoSlice::new(&[]); 64];
            assert_eq!(bufs.chunks_vectored(&mut vectors), 0);
            assert!(vectors.iter().all(|vector| vector.is_empty()));
        }
    }
}
