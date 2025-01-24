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
                let _ = self.bufs.remove(0);
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
                    return self.bufs.remove(0).unwrap().copy_to_bytes(len);
                }
                Ordering::Less => {}
            }
        }

        let mut bufs = BytesMut::with_capacity(len);
        bufs.put(self.take(len));
        let bufs = bufs.freeze();

        self.len -= len;
        bufs
    }
}
