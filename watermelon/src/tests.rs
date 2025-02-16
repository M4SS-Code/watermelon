use std::{sync::Arc, task::Waker};

use futures_util::task::ArcWake;

use crate::util::atomic::{AtomicUsize, Ordering};

#[derive(Debug)]
pub(crate) struct FlagWaker(AtomicUsize);

impl FlagWaker {
    pub(crate) fn new() -> (Arc<Self>, Waker) {
        let this = Arc::new(Self(AtomicUsize::new(0)));
        let waker = futures_util::task::waker(Arc::clone(&this));
        (this, waker)
    }

    pub(crate) fn wakes(&self) -> usize {
        self.0.load(Ordering::Acquire)
    }
}

impl ArcWake for FlagWaker {
    fn wake_by_ref(arc_self: &Arc<Self>) {
        arc_self.0.fetch_add(1, Ordering::AcqRel);
    }
}
