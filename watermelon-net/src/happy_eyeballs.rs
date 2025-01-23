use std::{
    future::Future,
    io,
    net::SocketAddr,
    pin::{pin, Pin},
    task::{Context, Poll},
    time::Duration,
};

use futures_util::{
    stream::{self, FusedStream, FuturesUnordered},
    Stream, StreamExt,
};
use pin_project_lite::pin_project;
use tokio::{
    net::{self, TcpStream},
    time::{self, Sleep},
};
use watermelon_proto::{Host, ServerAddr};

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
            let host = <_ as AsRef<str>>::as_ref(host);
            let addrs = net::lookup_host(format!("{}:{}", host, addr.port())).await?;

            let mut happy_eyeballs = pin!(HappyEyeballs::new(stream::iter(addrs)));
            let mut last_err = None;
            loop {
                match happy_eyeballs.next().await {
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
        dns: Option<D>,
        dns_received: Vec<SocketAddr>,
        connecting: FuturesUnordered<
            Pin<Box<dyn Future<Output = io::Result<TcpStream>> + Send + Sync + 'static>>,
        >,
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
            connecting: FuturesUnordered::new(),
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
    D: Stream<Item = SocketAddr> + Unpin,
{
    type Item = io::Result<TcpStream>;

    fn poll_next(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let mut this = self.project();
        let mut dead_end = true;

        while let Some(dns) = &mut this.dns {
            match Pin::new(&mut *dns).poll_next(cx) {
                Poll::Pending => {
                    dead_end = false;
                    break;
                }
                Poll::Ready(Some(record)) => {
                    dead_end = false;
                    this.dns_received.push(record);
                }
                Poll::Ready(None) => *this.dns = None,
            }
        }

        loop {
            match Pin::new(&mut this.connecting).poll_next(cx) {
                Poll::Pending => dead_end = false,
                Poll::Ready(Some(maybe_conn)) => return Poll::Ready(Some(maybe_conn)),
                Poll::Ready(None) => {}
            }

            let make_new_attempt = if this.connecting.is_empty() {
                true
            } else if let Some(next_attempt_delay) = this.next_attempt_delay.as_mut().as_pin_mut() {
                match next_attempt_delay.poll(cx) {
                    Poll::Pending => false,
                    Poll::Ready(()) => {
                        this.next_attempt_delay.set(None);
                        true
                    }
                }
            } else {
                true
            };
            if !make_new_attempt {
                break;
            }

            match this.next_dns_record() {
                Some(record) => {
                    let conn_fut = TcpStream::connect(record);
                    this.connecting.push(Box::pin(conn_fut));
                    this.next_attempt_delay
                        .set(Some(time::sleep(CONN_ATTEMPT_DELAY)));
                }
                None => break,
            }
        }

        if dead_end {
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
    D: Stream<Item = SocketAddr> + Unpin,
{
    fn is_terminated(&self) -> bool {
        self.dns.is_none() && self.dns_received.is_empty() && self.connecting.is_empty()
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
