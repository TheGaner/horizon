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
};
use tokio::{
    io::AsyncWriteExt,
    net::{TcpListener, TcpStream},
    task::JoinHandle,
};

const ALPN: &[u8] = b"horizon/friends-iroh/experimental/1";
const ACCEPT: &[u8; 2] = b"OK";
const SECRET_LENGTH: usize = 32;
const HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

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
        if !destination.ip().is_loopback() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "destination must be loopback",
            ));
        }
        let incoming = self
            .endpoint
            .accept()
            .await
            .ok_or(io::ErrorKind::ConnectionAborted)?;
        let connection = tokio::time::timeout(HANDSHAKE_TIMEOUT, incoming)
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        let (mut send, mut recv) = tokio::time::timeout(HANDSHAKE_TIMEOUT, connection.accept_bi())
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        let mut proposed = [0u8; SECRET_LENGTH];
        tokio::time::timeout(HANDSHAKE_TIMEOUT, recv.read_exact(&mut proposed))
            .await
            .map_err(|_| io::ErrorKind::TimedOut)?
            .map_err(other)?;
        // Constant-time equality, with no password-oracle error response.
        let mut difference = 0u8;
        for (a, b) in proposed.iter().zip(self.secret.iter()) {
            difference |= a ^ b;
        }
        if difference != 0 {
            connection.close(1u32.into(), b"unauthorized");
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                "unauthorized",
            ));
        }
        let tcp = TcpStream::connect(destination).await?;
        send.write_all(ACCEPT).await.map_err(other)?;
        let result = copy_to_tcp_and_back(&mut send, &mut recv, tcp).await;
        // A finished QUIC stream is not necessarily acknowledged yet. Wait
        // briefly for the peer's connection close so dropping the host's
        // endpoint cannot discard the last TCP response in flight.
        let _ = tokio::time::timeout(std::time::Duration::from_secs(5), connection.closed()).await;
        result
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
        let (mut send, mut recv) = conn.open_bi().await.map_err(other)?;
        send.write_all(&ticket.secret).await.map_err(other)?;
        let mut response = [0u8; 2];
        recv.read_exact(&mut response).await.map_err(other)?;
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
            let (tcp, _) = listener.accept().await?;
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
