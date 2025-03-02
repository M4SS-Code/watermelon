use std::{
    future::{self, Future},
    io,
    net::SocketAddr,
    pin::{Pin, pin},
    task::{Context, Poll},
    time::Duration,
};

use futures_core::{Stream, stream::FusedStream};
use pin_project_lite::pin_project;
use tokio::{
    net::{self, TcpStream},
    task::JoinSet,
    time::{self, Sleep},
};
use watermelon_proto::{Host, ServerAddr};

use crate::future::IterToStream;

const CONN_ATTEMPT_DELAY: Duration = Duration::from_millis(250);

/// Connects to an address and returns a [`TcpStream`].
///
/// If the given address is an ip, this just uses [`TcpStream::connect`]. Otherwise, if a host is
/// given, the [Happy Eyeballs] protocol is being used.
///
/// [Happy Eyeballs]: https://en.wikipedia.org/wiki/Happy_Eyeballs
///
/// # Errors
///
/// It returns an error if it is not possible to connect to any host.
pub async fn connect(addr: &ServerAddr) -> io::Result<TcpStream> {
    match addr.host() {
        Host::Ip(ip) => TcpStream::connect(SocketAddr::new(*ip, addr.port())).await,
        Host::Dns(host) => {
            let addrs = net::lookup_host((&**host, addr.port())).await?;

            let mut happy_eyeballs = pin!(HappyEyeballs::new(IterToStream { iter: addrs }));
            let mut last_err = None;
            loop {
                match future::poll_fn(|cx| happy_eyeballs.as_mut().poll_next(cx)).await {
                    Some(Ok(conn)) => return Ok(conn),
                    Some(Err(err)) => last_err = Some(err),
                    None => {
                        return Err(last_err.unwrap_or_else(|| {
                            io::Error::new(
                                io::ErrorKind::InvalidInput,
                                "could not resolve to any address",
                            )
                        }));
                    }
                }
            }
        }
    }
}

pin_project! {
    #[project = HappyEyeballsProj]
    struct HappyEyeballs<D> {
        #[pin]
        dns: Option<D>,
        dns_received: Vec<SocketAddr>,
        connecting: JoinSet<io::Result<TcpStream>>,
        last_attempted: Option<LastAttempted>,
        #[pin]
        next_attempt_delay: Option<Sleep>,
    }
}

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
enum LastAttempted {
    Ipv4,
    Ipv6,
}

impl<D> HappyEyeballs<D> {
    fn new(dns: D) -> Self {
        Self {
            dns: Some(dns),
            dns_received: Vec::new(),
            connecting: JoinSet::new(),
            last_attempted: None,
            next_attempt_delay: None,
        }
    }
}

impl<D> HappyEyeballsProj<'_, D> {
    fn next_dns_record(&mut self) -> Option<SocketAddr> {
        if self.dns_received.is_empty() {
            return None;
        }

        let next_kind = self
            .last_attempted
            .map_or(LastAttempted::Ipv6, LastAttempted::opposite);
        for i in 0..self.dns_received.len() {
            if LastAttempted::from_addr(self.dns_received[i]) == next_kind {
                *self.last_attempted = Some(next_kind);
                return Some(self.dns_received.remove(i));
            }
        }

        let record = self.dns_received.remove(0);
        *self.last_attempted = Some(LastAttempted::from_addr(record));
        Some(record)
    }
}

impl<D> Stream for HappyEyeballs<D>
where
    D: Stream<Item = SocketAddr>,
{
    type Item = io::Result<TcpStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();

        while let Some(dns) = this.dns.as_mut().as_pin_mut() {
            match dns.poll_next(cx) {
                Poll::Pending => break,
                Poll::Ready(Some(record)) => this.dns_received.push(record),
                Poll::Ready(None) => this.dns.set(None),
            }
        }

        loop {
            match this.connecting.poll_join_next(cx) {
                Poll::Pending => {
                    if let Some(next_attempt_delay) = this.next_attempt_delay.as_mut().as_pin_mut()
                    {
                        match next_attempt_delay.poll(cx) {
                            Poll::Pending => break,
                            Poll::Ready(()) => this.next_attempt_delay.set(None),
                        }
                    }
                }
                Poll::Ready(Some(maybe_conn)) => {
                    return Poll::Ready(Some(maybe_conn.expect("connect panicked")));
                }
                Poll::Ready(None) => {}
            }

            let Some(record) = this.next_dns_record() else {
                this.next_attempt_delay.set(None);
                break;
            };
            let conn_fut = TcpStream::connect(record);
            this.connecting.spawn(conn_fut);
            this.next_attempt_delay
                .set(Some(time::sleep(CONN_ATTEMPT_DELAY)));
        }

        if this.dns.is_none() && this.connecting.is_empty() && this.next_attempt_delay.is_none() {
            Poll::Ready(None)
        } else {
            Poll::Pending
        }
    }

    fn size_hint(&self) -> (usize, Option<usize>) {
        let (mut len, mut max) = self.dns.as_ref().map_or((0, Some(0)), Stream::size_hint);
        len = len.saturating_add(self.dns_received.len() + self.connecting.len());
        if let Some(max) = &mut max {
            *max = max.saturating_add(self.dns_received.len() + self.connecting.len());
        }
        (len, max)
    }
}

impl<D> FusedStream for HappyEyeballs<D>
where
    D: Stream<Item = SocketAddr>,
{
    fn is_terminated(&self) -> bool {
        self.dns.is_none() && self.connecting.is_empty() && self.next_attempt_delay.is_none()
    }
}

impl LastAttempted {
    fn from_addr(addr: SocketAddr) -> Self {
        match addr {
            SocketAddr::V4(_) => Self::Ipv4,
            SocketAddr::V6(_) => Self::Ipv6,
        }
    }

    fn opposite(self) -> Self {
        match self {
            Self::Ipv4 => Self::Ipv6,
            Self::Ipv6 => Self::Ipv4,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::{
        net::{Ipv4Addr, Ipv6Addr},
        pin::pin,
    };

    use futures_util::{StreamExt as _, stream};
    use tokio::net::TcpListener;

    use super::HappyEyeballs;

    #[tokio::test]
    async fn happy_eyeballs_prefer_v6() {
        let ipv4_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let ipv6_listener = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await.unwrap();

        let ipv4_addr = ipv4_listener.local_addr().unwrap();
        let ipv6_addr = ipv6_listener.local_addr().unwrap();

        tokio::spawn(async move { while ipv6_listener.accept().await.is_ok() {} });

        let addrs = stream::iter([ipv4_addr, ipv6_addr]);
        let mut happy_eyeballs = pin!(HappyEyeballs::new(addrs));
        let conn = happy_eyeballs.next().await.unwrap().unwrap();
        assert!(conn.peer_addr().unwrap().is_ipv6());
    }

    #[tokio::test]
    async fn happy_eyeballs_fallback_v4() {
        let ipv4_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();
        let ipv6_listener = TcpListener::bind((Ipv6Addr::LOCALHOST, 0)).await.unwrap();

        let ipv4_addr = ipv4_listener.local_addr().unwrap();
        let ipv6_addr = ipv6_listener.local_addr().unwrap();

        drop(ipv6_listener);
        tokio::spawn(async move { while ipv4_listener.accept().await.is_ok() {} });

        let addrs = stream::iter([ipv4_addr, ipv6_addr]);
        let mut happy_eyeballs = pin!(HappyEyeballs::new(addrs));
        let _v6_failure = happy_eyeballs.next().await;
        let conn = happy_eyeballs.next().await.unwrap().unwrap();
        assert!(conn.peer_addr().unwrap().is_ipv4());
    }

    #[tokio::test]
    async fn happy_eyeballs_only_v4_available() {
        let ipv4_listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await.unwrap();

        let ipv4_addr = ipv4_listener.local_addr().unwrap();

        tokio::spawn(async move { while ipv4_listener.accept().await.is_ok() {} });

        let addrs = stream::iter([ipv4_addr]);
        let mut happy_eyeballs = pin!(HappyEyeballs::new(addrs));
        let conn = happy_eyeballs.next().await.unwrap().unwrap();
        assert!(conn.peer_addr().unwrap().is_ipv4());
    }
}
