use std::{collections::BTreeSet, sync::Arc};

use arc_swap::ArcSwap;
use tokio::sync::mpsc;
use watermelon_proto::{ServerInfo, Subject};

use crate::{
    client::{create_inbox_subject, RawQuickInfo},
    handler::HandlerCommand,
};

#[derive(Debug)]
pub(crate) struct TestHandler {
    pub(crate) receiver: mpsc::Receiver<HandlerCommand>,
    pub(crate) _info: Arc<ArcSwap<ServerInfo>>,
    pub(crate) quick_info: Arc<RawQuickInfo>,
}

#[test]
fn unique_create_inbox_subject() {
    const ITERATIONS: usize = if cfg!(miri) { 100 } else { 100_000 };

    let prefix = Subject::from_static("abcd");
    let subjects = (0..ITERATIONS)
        .map(|_| create_inbox_subject(&prefix))
        .collect::<BTreeSet<_>>();
    assert_eq!(subjects.len(), ITERATIONS);
}
