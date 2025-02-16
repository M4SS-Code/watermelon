use std::{
    sync::Arc,
    task::{Wake, Waker},
};

use crate::util::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
pub(crate) struct FlagWaker(AtomicUsize);

impl FlagWaker {
    pub(crate) fn new() -> (Arc<Self>, Waker) {
        let this = Arc::new(Self(AtomicUsize::new(0)));
        let waker = Waker::from(Arc::clone(&this));
        (this, waker)
    }

    pub(crate) fn wakes(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

impl Wake for FlagWaker {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::AcqRel);
    }
}
