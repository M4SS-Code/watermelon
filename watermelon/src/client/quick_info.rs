use crate::util::atomic::{AtomicU32, Ordering};

const IS_CONNECTED: u32 = 1 << 0;
#[cfg(feature = "non-standard-zstd")]
const IS_ZSTD_COMPRESSED: u32 = 1 << 1;
const IS_LAMEDUCK: u32 = 1 << 2;
const IS_FAILED_UNSUBSCRIBE: u32 = 1 << 31;

#[derive(Debug)]
pub(crate) struct RawQuickInfo(AtomicU32);

/// Client information
///
/// Obtained from [`Client::quick_info`].
///
/// [`Client::quick_info`]: crate::core::Client::quick_info
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
#[cfg_attr(feature = "non-standard-zstd", expect(clippy::struct_excessive_bools))]
pub struct QuickInfo {
    pub(crate) is_connected: bool,
    #[cfg(feature = "non-standard-zstd")]
    pub(crate) is_zstd_compressed: bool,
    pub(crate) is_lameduck: bool,
    pub(crate) is_failed_unsubscribe: bool,
}

impl RawQuickInfo {
    pub(crate) fn new() -> Self {
        Self(AtomicU32::new(
            QuickInfo {
                is_connected: false,
                #[cfg(feature = "non-standard-zstd")]
                is_zstd_compressed: false,
                is_lameduck: false,
                is_failed_unsubscribe: false,
            }
            .encode(),
        ))
    }

    pub(crate) fn get(&self) -> QuickInfo {
        QuickInfo::decode(self.0.load(Ordering::Acquire))
    }

    pub(crate) fn store<F>(&self, mut f: F)
    where
        F: FnMut(QuickInfo) -> QuickInfo,
    {
        let prev_params = self.get();
        self.0.store(f(prev_params).encode(), Ordering::Release);
    }

    pub(crate) fn store_is_connected(&self, val: bool) {
        self.store_bit(IS_CONNECTED, val);
    }
    pub(crate) fn store_is_lameduck(&self, val: bool) {
        self.store_bit(IS_LAMEDUCK, val);
    }
    pub(crate) fn store_is_failed_unsubscribe(&self, val: bool) {
        self.store_bit(IS_FAILED_UNSUBSCRIBE, val);
    }

    #[expect(
        clippy::inline_always,
        reason = "we want this to be inlined inside the store_* functions"
    )]
    #[inline(always)]
    fn store_bit(&self, mask: u32, val: bool) {
        debug_assert_eq!(mask.count_ones(), 1);

        if val {
            self.0.fetch_or(mask, Ordering::AcqRel);
        } else {
            self.0.fetch_and(!mask, Ordering::AcqRel);
        }
    }
}

impl QuickInfo {
    /// Returns `true` if the client is currently connected to the NATS server
    #[must_use]
    pub fn is_connected(&self) -> bool {
        self.is_connected
    }

    /// Returns `true` if the client connection is zstd compressed
    #[cfg(feature = "non-standard-zstd")]
    #[must_use]
    pub fn is_zstd_compressed(&self) -> bool {
        self.is_zstd_compressed
    }

    /// Returns `true` if the client is currently in Lame Duck Mode
    #[must_use]
    pub fn is_lameduck(&self) -> bool {
        self.is_lameduck
    }

    fn encode(self) -> u32 {
        let mut val = 0;

        if self.is_connected {
            val |= IS_CONNECTED;
        }

        #[cfg(feature = "non-standard-zstd")]
        if self.is_zstd_compressed {
            val |= IS_ZSTD_COMPRESSED;
        }

        if self.is_lameduck {
            val |= IS_LAMEDUCK;
        }

        if self.is_failed_unsubscribe {
            val |= IS_FAILED_UNSUBSCRIBE;
        }

        val
    }

    fn decode(val: u32) -> Self {
        Self {
            is_connected: (val & IS_CONNECTED) != 0,
            #[cfg(feature = "non-standard-zstd")]
            is_zstd_compressed: (val & IS_ZSTD_COMPRESSED) != 0,
            is_lameduck: (val & IS_LAMEDUCK) != 0,
            is_failed_unsubscribe: (val & IS_FAILED_UNSUBSCRIBE) != 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{QuickInfo, RawQuickInfo};

    #[test]
    fn set_get() {
        let quick_info = RawQuickInfo::new();
        let mut expected = QuickInfo {
            is_connected: false,
            #[cfg(feature = "non-standard-zstd")]
            is_zstd_compressed: false,
            is_lameduck: false,
            is_failed_unsubscribe: false,
        };

        for is_connected in [false, true] {
            quick_info.store_is_connected(is_connected);
            expected.is_connected = is_connected;

            for is_lameduck in [false, true] {
                quick_info.store_is_lameduck(is_lameduck);
                expected.is_lameduck = is_lameduck;

                for is_failed_unsubscribe in [false, true] {
                    quick_info.store_is_failed_unsubscribe(is_failed_unsubscribe);
                    expected.is_failed_unsubscribe = is_failed_unsubscribe;

                    assert_eq!(expected, quick_info.get());
                }
            }
        }
    }
}
