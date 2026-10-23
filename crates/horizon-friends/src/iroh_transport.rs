//! Experimental, opt-in native Iroh QUIC transport for a single Minecraft TCP
//! connection. Does NOT change existing friend invites or automatic paths.
//!
//! Iroh authenticates the server EndpointId with QUIC TLS; an independent,
//! unguessable invite secret is required before the remote may connect to
//! the local Minecraft server. A private/relay-only mode must never use a
//! direct local ticket: callers must apply the transport privacy policy.
//! This module deliberately uses no default public relay or name service.

use crate::PathMode;
use iroh::{
    Endpoint, EndpointAddr, TransportAddr,
    endpoint::{PortmapperConfig, presets},
};
use std::{
    io,
    net::{Ipv4Addr, SocketAddr},
    num::NonZeroUsize,
    sync::Arc,
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    sync::{Semaphore, watch},
    task::{JoinHandle, JoinSet},
};

const ALPN: &[u8] = b"horizon/friends-iroh/experimental/1";
const ACCEPT: &[u8; 2] = b"OK";
const SECRET_LENGTH: usize = 32;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);
const SHUTDOWN_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

/// Aggregated experimental QUIC host metrics. Never includes endpoint secrets.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct HostStats {
    pub completed: u64,
    pub denied: u64,
    pub failed: u64,
    pub at_capacity: u64,
    pub cancelled: u64,
}

fn validate_destination(destination: SocketAddr) -> io::Result<()> {
    if destination.ip().is_loopback() {
        Ok(())
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "destination must be loopback",
        ))
    }
}

async fn accept_and_proxy(
    connection: iroh::endpoint::Connection,
    destination: SocketAddr,
    secret: [u8; SECRET_LENGTH],
) -> io::Result<()> {
    let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
        .map_err(other)?;
    let mut proposed = [0u8; SECRET_LENGTH];
    tokio::time::timeout(HANDSHAKE_TIMEOUT, recv.read_exact(&mut proposed))
        .await
        .map_err(|_| io::ErrorKind::TimedOut)?
        .map_err(other)?;
    // Constant-time compare without opening any local TCP connection first.
    let mut difference = 0u8;
    for (a, b) in proposed.iter().zip(secret.iter()) {
        difference |= a ^ b;
    }
    if difference != 0 {
        connection.close(1u32.into(), b"unauthorized");
        return Err(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "unauthorized",
        ));
    }
    let tcp = tokio::time::timeout(HANDSHAKE_TIMEOUT, TcpStream::connect(destination))
        .await
        .map_err(|_| io::ErrorKind::TimedOut)??;
    send.write_all(ACCEPT).await.map_err(other)?;
    let result = copy_to_tcp_and_back(&mut send, &mut recv, tcp).await;
    // Flush end-of-stream and await a bounded guest close before letting the
    // owning endpoint shut down: avoids losing the final Minecraft reply.
    let _ = tokio::time::timeout(SHUTDOWN_GRACE, connection.closed()).await;
    result
}

fn record_result(stats: &mut HostStats, result: Result<io::Result<()>, tokio::task::JoinError>) {
    match result {
        Ok(Ok(())) => stats.completed += 1,
        Ok(Err(e)) if e.kind() == io::ErrorKind::PermissionDenied => stats.denied += 1,
        _ => stats.failed += 1,
    }
}

/// Invitation capability; do not log or expose the secret inadvertently.
#[derive(Clone)]
pub struct Ticket {
    pub endpoint: EndpointAddr,
    secret: [u8; SECRET_LENGTH],
}

impl Ticket {
    pub fn new(endpoint: EndpointAddr, secret: [u8; SECRET_LENGTH]) -> Self {
        Self { endpoint, secret }
    }
    /// Internal exposure only for the authenticated, versioned invitation
    /// codec. Do not expose this capability in Debug/log output.
    pub(crate) fn capability(&self) -> &[u8; SECRET_LENGTH] {
        &self.secret
    }
}

/// An experimental QUIC host; the TCP destination is always explicitly local.
pub struct Host {
    endpoint: Endpoint,
    secret: [u8; SECRET_LENGTH],
}

impl Host {
    /// Generate a cryptographically random bearer capability for this share.
    pub async fn bind_local_random() -> io::Result<Self> {
        let mut secret = [0u8; SECRET_LENGTH];
        crate::fill_random(&mut secret)?;
        Self::bind_local(secret).await
    }

    /// `secret` must be uniformly random; prefer `bind_local_random`.
    pub async fn bind_local(secret: [u8; SECRET_LENGTH]) -> io::Result<Self> {
        let endpoint = Endpoint::builder(presets::Minimal)
            .alpns(vec![ALPN.to_vec()])
            .bind_addr("127.0.0.1:0")
            .map_err(other)?
            .bind()
            .await
            .map_err(other)?;
        Ok(Self { endpoint, secret })
    }

    pub fn ticket(&self) -> Ticket {
        Ticket::new(self.endpoint.addr(), self.secret)
    }

    /// Accept and forward one QUIC connection. A failed secret must not
    /// create any TCP connection to the integrated Minecraft server.
    pub async fn serve_one(&self, destination: SocketAddr) -> io::Result<()> {
        validate_destination(destination)?;
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or(io::ErrorKind::ConnectionAborted)?;
        let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming)
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        accept_and_proxy(connection, destination, self.secret).await
    }

    /// Serve multiple authenticated guests on one Iroh endpoint, bounding
    /// concurrent handshake + TCP sessions and cleanly stopping on request.
    /// No public relays or NAT traversal are enabled by this localhost API.
    pub async fn serve_many(
        &self,
        destination: SocketAddr,
        max_guests: NonZeroUsize,
        mut shutdown: watch::Receiver<bool>,
    ) -> io::Result<HostStats> {
        validate_destination(destination)?;
        let permits = Arc::new(Semaphore::new(max_guests.get()));
        let mut tasks = JoinSet::new();
        let mut stats = HostStats::default();
        while !*shutdown.borrow() {
            tokio::select! {
                biased;
                changed = shutdown.changed() => {
                    if changed.is_err() || *shutdown.borrow() { break; }
                }
                done = tasks.join_next(), if !tasks.is_empty() => {
                    if let Some(result) = done { record_result(&mut stats, result); }
                }
                incoming = self.endpoint.accept() => {
                    let incoming = incoming.ok_or(io::ErrorKind::ConnectionAborted)?;
                    let Ok(permit) = permits.clone().try_acquire_owned() else {
                        incoming.refuse();
                        stats.at_capacity += 1;
                        continue;
                    };
                    let secret = self.secret;
                    tasks.spawn(async move {
                        let _permit = permit;
                        let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming)
                            .await.map_err(|_| io::ErrorKind::TimedOut)?.map_err(other)?;
                        accept_and_proxy(connection, destination, secret).await
                    });
                }
            }
        }
        let drained = tokio::time::timeout(SHUTDOWN_GRACE, async {
            while let Some(result) = tasks.join_next().await {
                record_result(&mut stats, result);
            }
        })
        .await
        .is_ok();
        if !drained {
            stats.cancelled += tasks.len() as u64;
            tasks.abort_all();
            while tasks.join_next().await.is_some() {}
        }
        Ok(stats)
    }

    pub async fn close(self) {
        self.endpoint.close().await;
    }
}

pub struct Guest {
    endpoint: Endpoint,
    send: iroh::endpoint::SendStream,
    recv: iroh::endpoint::RecvStream,
}

impl Guest {
    /// Experimental direct-only connection. Does not enable public relays.
    pub async fn connect(ticket: &Ticket) -> io::Result<Self> {
        Self::connect_with_mode(ticket, PathMode::DirectOnly).await
    }

    /// A fail-closed path selector for the experimental native QUIC client.
    /// There is no independently operated Iroh relay configured yet, so
    /// RelayOnly must refuse before binding UDP or advertising local IPs.
    /// Auto currently selects only explicit direct candidates, never an
    /// unapproved third-party default relay. Do not use this experimental
    /// method as production routing until a relay policy is implemented.
    pub async fn connect_with_mode(ticket: &Ticket, mode: PathMode) -> io::Result<Self> {
        if mode == PathMode::RelayOnly {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "Iroh relay-only mode requires a configured, trusted relay; direct UDP is forbidden",
            ));
        }
        let direct: Vec<_> = ticket
            .endpoint
            .ip_addrs()
            .copied()
            .filter(|addr| addr.port() != 0)
            .map(TransportAddr::Ip)
            .collect();
        if direct.is_empty() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "No direct Iroh address provided; relay fallback is not configured",
            ));
        }
        let endpoint_addr = EndpointAddr::from_parts(ticket.endpoint.id, direct);
        let endpoint = Endpoint::builder(presets::Minimal)
            .clear_relay_transports()
            .clear_address_lookup()
            .portmapper_config(PortmapperConfig::Disabled)
            .bind()
            .await
            .map_err(other)?;
        let conn = tokio::time::timeout(HANDSHAKE_TIMEOUT, endpoint.connect(endpoint_addr, ALPN))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, conn.open_bi())
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        tokio::time::timeout(HANDSHAKE_TIMEOUT, send.write_all(&ticket.secret))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        let mut response = [0u8; 2];
        tokio::time::timeout(HANDSHAKE_TIMEOUT, recv.read_exact(&mut response))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        if &response != ACCEPT {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "host did not accept invitation",
            ));
        }
        Ok(Self {
            endpoint,
            send,
            recv,
        })
    }

    /// Provide an ordinary loopback TCP port to Horizon's existing client.
    pub async fn serve_loopback(self) -> io::Result<(u16, JoinHandle<io::Result<()>>)> {
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let port = listener.local_addr()?.port();
        let task = tokio::spawn(async move {
            // A cancelled join must not keep a local socket and QUIC endpoint
            // alive forever when the Minecraft client never dials this port.
            let (tcp, _) = tokio::time::timeout(HANDSHAKE_TIMEOUT, listener.accept())
                .await
                .map_err(|_| io::ErrorKind::TimedOut)??;
            let Guest {
                endpoint,
                mut send,
                mut recv,
            } = self;
            let result = copy_to_tcp_and_back(&mut send, &mut recv, tcp).await;
            endpoint.close().await;
            result
        });
        Ok((port, task))
    }
}

async fn copy_to_tcp_and_back(
    send: &mut iroh::endpoint::SendStream,
    recv: &mut iroh::endpoint::RecvStream,
    tcp: TcpStream,
) -> io::Result<()> {
    let (mut from_tcp, mut to_tcp) = tcp.into_split();
    let tcp_to_quic = async {
        tokio::io::copy(&mut from_tcp, send).await?;
        send.finish().map_err(other)?;
        Ok::<_, io::Error>(())
    };
    let quic_to_tcp = async {
        tokio::io::copy(recv, &mut to_tcp).await?;
        to_tcp.shutdown().await?;
        Ok::<_, io::Error>(())
    };
    tokio::try_join!(tcp_to_quic, quic_to_tcp)?;
    Ok(())
}

fn other(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;
    use tokio::io::AsyncReadExt;

    #[tokio::test]
    async fn authenticated_quic_carries_minecraft_tcp_without_public_services() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let server_addr = tcp.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut conn, _) = tcp.accept().await.unwrap();
            let mut input = [0u8; 4];
            conn.read_exact(&mut input).await.unwrap();
            assert_eq!(&input, b"PING");
            conn.write_all(b"PONG").await.unwrap();
        });
        let host = Host::bind_local([23u8; SECRET_LENGTH]).await.unwrap();
        // Exercise the real QUIC path using a decoded V3 invitation, not
        // merely an in-memory ticket, without routing through game UI.
        let ticket =
            crate::iroh_invite::decode(&crate::iroh_invite::encode(&host.ticket()).unwrap())
                .unwrap();
        let hosting = tokio::spawn(async move {
            let result = host.serve_one(server_addr).await;
            host.close().await;
            result
        });
        let guest = Guest::connect_with_mode(&ticket, PathMode::DirectOnly)
            .await
            .unwrap();
        let (port, relay) = guest.serve_loopback().await.unwrap();
        let mut local = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        local.write_all(b"PING").await.unwrap();
        local.shutdown().await.unwrap();
        let mut got = Vec::new();
        local.read_to_end(&mut got).await.unwrap();
        assert_eq!(got, b"PONG");
        tokio::time::timeout(Duration::from_secs(5), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), hosting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        server.await.unwrap();
    }

    #[tokio::test]
    async fn relay_only_fails_closed_before_creating_any_udp_endpoint() {
        let host = Host::bind_local([61u8; SECRET_LENGTH]).await.unwrap();
        let ticket = host.ticket();
        let result = Guest::connect_with_mode(&ticket, PathMode::RelayOnly).await;
        assert!(matches!(result, Err(ref e) if e.kind() == io::ErrorKind::PermissionDenied));
        host.close().await;
    }

    #[tokio::test]
    async fn direct_only_never_attempts_to_use_an_invited_relay_without_ip_candidates() {
        let host = Host::bind_local([62u8; SECRET_LENGTH]).await.unwrap();
        let mut ticket = host.ticket();
        ticket.endpoint = EndpointAddr::new(ticket.endpoint.id)
            .with_relay_url("https://relay.invalid.example".parse().unwrap());
        let result = Guest::connect_with_mode(&ticket, PathMode::DirectOnly).await;
        assert!(matches!(result, Err(ref e) if e.kind() == io::ErrorKind::InvalidInput));
        host.close().await;
    }

    #[tokio::test]
    async fn guest_times_out_when_host_never_acknowledges_capability() {
        let host = Host::bind_local([59u8; SECRET_LENGTH]).await.unwrap();
        let ticket = host.ticket();
        let stuck = tokio::spawn(async move {
            let incoming = host.endpoint.accept().await.unwrap();
            let connection = incoming.await.unwrap();
            let (_send, mut recv) = connection.accept_bi().await.unwrap();
            let mut sent = [0u8; SECRET_LENGTH];
            recv.read_exact(&mut sent).await.unwrap();
            assert_eq!(sent, [59u8; SECRET_LENGTH]);
            tokio::time::sleep(Duration::from_secs(7)).await;
            host.close().await;
        });
        let error = match Guest::connect(&ticket).await {
            Ok(_) => panic!("guest accepted a host that never acknowledged"),
            Err(error) => error,
        };
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        stuck.abort();
        let _ = stuck.await;
    }

    #[tokio::test]
    async fn abandoned_guest_local_port_returns_timeout_instead_of_waiting_forever() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = tcp.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (stream, _) = tcp.accept().await.unwrap();
            let mut data = Vec::new();
            let _ = tokio::time::timeout(Duration::from_secs(8), async {
                let mut stream = stream;
                stream.read_to_end(&mut data).await
            })
            .await;
        });
        let host = Host::bind_local([63u8; SECRET_LENGTH]).await.unwrap();
        let ticket = host.ticket();
        let hosting = tokio::spawn(async move {
            let result = host.serve_one(destination).await;
            host.close().await;
            result
        });
        let guest = Guest::connect(&ticket).await.unwrap();
        let (_port, relay) = guest.serve_loopback().await.unwrap();
        let error = tokio::time::timeout(Duration::from_secs(8), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        // No local client ever connected, so we are checking teardown, not a
        // Minecraft application reply or success path.
        let _ = tokio::time::timeout(Duration::from_secs(8), hosting).await;
        let _ = tokio::time::timeout(Duration::from_secs(8), server).await;
    }

    #[tokio::test]
    async fn multi_guest_bad_ticket_then_two_valid_clients() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let mut sessions = JoinSet::new();
            for _ in 0..2 {
                let (mut conn, _) = tcp.accept().await.unwrap();
                sessions.spawn(async move {
                    let mut request = [0u8; 4];
                    conn.read_exact(&mut request).await.unwrap();
                    assert_eq!(&request, b"PING");
                    conn.write_all(b"PONG").await.unwrap();
                });
            }
            while let Some(r) = sessions.join_next().await {
                r.unwrap();
            }
        });
        let host = Host::bind_local([43u8; SECRET_LENGTH]).await.unwrap();
        let ticket =
            crate::iroh_invite::decode(&crate::iroh_invite::encode(&host.ticket()).unwrap())
                .unwrap();
        let (stop, rx) = watch::channel(false);
        let hosted = tokio::spawn(async move {
            let stats = host
                .serve_many(address, NonZeroUsize::new(3).unwrap(), rx)
                .await;
            host.close().await;
            stats
        });
        let mut invalid = ticket.clone();
        invalid.secret = [44u8; SECRET_LENGTH];
        let denied = Guest::connect(&invalid).await;
        assert!(denied.is_err());
        let mut clients = JoinSet::new();
        for _ in 0..2 {
            let ticket = ticket.clone();
            clients.spawn(async move {
                let guest = Guest::connect(&ticket).await.unwrap();
                let (port, relay) = guest.serve_loopback().await.unwrap();
                let mut stream = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
                    .await
                    .unwrap();
                stream.write_all(b"PING").await.unwrap();
                stream.shutdown().await.unwrap();
                let mut reply = Vec::new();
                stream.read_to_end(&mut reply).await.unwrap();
                assert_eq!(reply, b"PONG");
                relay.await.unwrap().unwrap();
            });
        }
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(result) = clients.join_next().await {
                result.unwrap();
            }
            server.await.unwrap();
        })
        .await
        .unwrap();
        stop.send(true).unwrap();
        let stats = tokio::time::timeout(Duration::from_secs(10), hosted)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stats.completed, 2);
        assert_eq!(stats.denied, 1);
        assert_eq!(stats.cancelled, 0);
    }

    #[tokio::test]
    async fn silent_handshake_releases_slot_and_next_guest_succeeds() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let destination = tcp.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut stream, _) = tcp.accept().await.unwrap();
            let mut request = [0u8; 4];
            stream.read_exact(&mut request).await.unwrap();
            assert_eq!(&request, b"PING");
            stream.write_all(b"PONG").await.unwrap();
        });
        let host = Host::bind_local([72u8; SECRET_LENGTH]).await.unwrap();
        let ticket = host.ticket();
        let (stop, rx) = watch::channel(false);
        let hosted = tokio::spawn(async move {
            let result = host
                .serve_many(destination, NonZeroUsize::new(1).unwrap(), rx)
                .await;
            host.close().await;
            result
        });
        let silent_endpoint = Endpoint::builder(presets::Minimal)
            .clear_relay_transports()
            .clear_address_lookup()
            .portmapper_config(PortmapperConfig::Disabled)
            .bind()
            .await
            .unwrap();
        let silent_connection = silent_endpoint
            .connect(ticket.endpoint.clone(), ALPN)
            .await
            .unwrap();
        // Peer deliberately opens no stream and supplies no capability.
        // Host must time out its accept_bi and release the semaphore permit.
        tokio::time::sleep(Duration::from_secs(6)).await;
        let guest = Guest::connect(&ticket).await.unwrap();
        let (port, relay) = guest.serve_loopback().await.unwrap();
        let mut local = TcpStream::connect((Ipv4Addr::LOCALHOST, port))
            .await
            .unwrap();
        local.write_all(b"PING").await.unwrap();
        local.shutdown().await.unwrap();
        let mut response = Vec::new();
        local.read_to_end(&mut response).await.unwrap();
        assert_eq!(response, b"PONG");
        tokio::time::timeout(Duration::from_secs(6), relay)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        tokio::time::timeout(Duration::from_secs(6), server)
            .await
            .unwrap()
            .unwrap();
        stop.send(true).unwrap();
        let stats = tokio::time::timeout(Duration::from_secs(8), hosted)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stats.completed, 1);
        assert_eq!(
            stats.failed, 1,
            "silent QUIC handshake should time out once"
        );
        assert_eq!(stats.cancelled, 0);
        drop(silent_connection);
        silent_endpoint.close().await;
    }

    #[tokio::test]
    async fn multi_guest_capacity_rejects_extra_connection_and_stops_bounded() {
        let tcp = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = tcp.local_addr().unwrap();
        let server = tokio::spawn(async move {
            let (mut active, _) = tcp.accept().await.unwrap();
            let mut sink = Vec::new();
            let _ = active.read_to_end(&mut sink).await;
        });
        let host = Host::bind_local([48u8; SECRET_LENGTH]).await.unwrap();
        let ticket = host.ticket();
        let (stop, rx) = watch::channel(false);
        let hosted = tokio::spawn(async move {
            let result = host
                .serve_many(address, NonZeroUsize::new(1).unwrap(), rx)
                .await;
            host.close().await;
            result
        });
        // Connecting the first guest establishes the TCP stream and occupies
        // the only semaphore permit for as long as its QUIC stream is open.
        let first = Guest::connect(&ticket).await.unwrap();
        let extra = Guest::connect(&ticket).await;
        assert!(extra.is_err(), "extra guest must be refused at capacity");
        stop.send(true).unwrap();
        let stats = tokio::time::timeout(Duration::from_secs(9), hosted)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert!(
            stats.at_capacity >= 1,
            "saturated arrivals must be accounted for"
        );
        assert!(
            stats.cancelled >= 1,
            "shutdown must abort a stalled active stream"
        );
        drop(first);
        tokio::time::timeout(Duration::from_secs(5), server)
            .await
            .unwrap()
            .unwrap();
    }

    #[tokio::test]
    async fn multi_guest_can_stop_while_idle_without_leaking_tasks() {
        let host = Host::bind_local([52u8; SECRET_LENGTH]).await.unwrap();
        let (stop, rx) = watch::channel(false);
        let hosting = tokio::spawn(async move {
            let result = host
                .serve_many(
                    "127.0.0.1:18888".parse().unwrap(),
                    NonZeroUsize::new(1).unwrap(),
                    rx,
                )
                .await;
            host.close().await;
            result
        });
        stop.send(true).unwrap();
        let stats = tokio::time::timeout(Duration::from_secs(3), hosting)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        assert_eq!(stats, HostStats::default());
    }

    #[tokio::test]
    async fn wrong_invite_never_reaches_the_tcp_server() {
        let server = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = server.local_addr().unwrap();
        let host = Host::bind_local([27u8; SECRET_LENGTH]).await.unwrap();
        let mut ticket = host.ticket();
        ticket.secret = [28u8; SECRET_LENGTH];
        let hosting = tokio::spawn(async move {
            let result = host.serve_one(addr).await;
            host.close().await;
            result
        });
        let result = Guest::connect(&ticket).await;
        assert!(result.is_err());
        assert_eq!(
            hosting.await.unwrap().unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert!(
            tokio::time::timeout(Duration::from_millis(100), server.accept())
                .await
                .is_err()
        );
    }
}
