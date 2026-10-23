# Friends network transport modernization — engineering design

**Status (2026-10-23):** first stage implemented on isolated branch
`feature/friends-iroh-transport-20261023`, based on H001 `4680a73f`.
The authoritative master roadmap remains
`/home/pc/Projects/horizon-parity-performance/reports/MASTER_ROADMAP.md`. This document
is a technical design and handoff, **not a second roadmap**.

## Baseline and principles

- `horizon-friends` currently carries vanilla TCP through an authenticated
  SPAKE2 (invite-secret) handshake and ChaCha20-Poly1305 framed stream.
  `guest.rs`, `host.rs`, `session.rs`, `crypto.rs`, `direct.rs` are all
  written for synchronous `TcpStream`; `horizon-relay` accepts plain TCP
  control and bridges E2E encrypted data. Retain this working fallback.
- Direct connectivity currently probes interface addresses, router PCP /
  NAT-PMP / UPnP mapping, STUN public IPv4 and TCP simultaneous-open.
  These are not interchangeable with UDP hole punching.
- Preserve backward-compatible `Invite` version 2 decoding while developing
  any richer candidate or identity format. **Never silently interpret new
  QUIC addresses as TCP hostnames.** Changes to sharing identity need an
  explicit wire/invite format version and regression fixtures.
- Privacy: endpoint candidates in long codes identify the host. Relay-only
  mode is an explicit privacy choice; never initiate direct P2P attempts
  when selected. The existing SPAKE2 secret is access authorization;
  Iroh endpoint TLS identity alone is not a substitute for that authorization.

## Completed initial slice — NET-01 (relay pool)

The setting `HORIZON_RELAY_URL` / `friendsRelay` accepts up to eight ordered
semicolon-separated relay endpoints. The host registers with the first
available relay, rotates when a registration or connection fails, publishes
only its active address in full invitation codes, and applies exponential
backoff only after trying the full list. Short-code guests can use an
explicitly trusted matching pool. Legacy single-address behavior remains.
This is **configured failover, not automatic public relay discovery**.

Relevant files:
`crates/horizon-friends/src/{host,guest,lib}.rs`,
`crates/horizon-relay/tests/tunnel.rs`, `docs/play-with-friends.md`.

Verification: run pinned Rust environment (`. ../scripts/horizon_env.sh`),
then `cargo test --locked -p horizon-friends --lib` and
`cargo test --locked -p horizon-relay --test tunnel -- --test-threads=4`.
These validate mock NAT and encryption code plus real loopback host/guest
sessions, including unreachable-primary-to-working-secondary failover.
Cross-internet, CGNAT and real-network latency remain **untested**.

## Follow-on design — NET-02 native Iroh QUIC

**Dependency gate:** Horizon workspace pins `rustc 1.90.0` via
`../scripts/horizon_env.sh`; current `n0-computer/iroh` 1.x manifests require
`rust-version = "1.91"` (verified against v1.3.0 and latest main on
2026-10-23). Do not patch away the MSRV, silently select an old protocol,
or change the globally shared Rust toolchain while upstream review jobs run.
Upgrade toolchain in a coordinated, isolated build environment first and
prove full workspace builds before changing release requirements.

**Integration sequence:**

1. Introduce a duplex transport adapter with bounded buffering and a safe
   shutdown contract. Keep the current TCP implementation as default. QUIC
   streams are asynchronous; avoid blocking Tokio tasks and avoid a
   per-packet thread bridge. Tests must cover EOF, backpressure, cancellation,
   half-close and resource exhaustion.
2. Add a native `iroh` endpoint (Rust crate, **not** e4mc Java/JNI/Netty).
   Bind the endpoint public key or authenticated certificate to invitation
   material and retain invite-secret authorization. Use a dedicated ALPN
   and protocol negotiation. Never accept arbitrary incoming peers as an
   unrestricted local-server proxy.
3. Extend invites with a versioned endpoint description. Keep v2 TCP invites
   and relay-only privacy policy. Establish QUIC via UDP hole punching and
   Iroh relay fallback; compare connect paths and migrate only when safe.
4. Maintain and test modes: TCP-only, QUIC-preferred, direct-only, relay-only
   and automatic. No connection may bypass explicit relay-only settings.
5. Add telemetry limited to connection route, handshake timing, hop RTT and
   failures; avoid logging invitation secrets, raw tickets or public IPs.

## NET-03 relay discovery and usability

A public relay broker requires **operator-provided infrastructure**. e4mc's
broker and domains are not implied public entitlements from its MIT license.
Implement signed/validated relay directory metadata, regional reachability
and a client-side RTT selection policy, with an explicit override to self-host
or disable relays. Benchmark under comparable realistic networks rather than
assuming QUIC always reduces latency or TCP is always slower.

## Decision references and licenses

- Horizon existing implementation: `docs/play-with-friends.md` and
  `crates/horizon-friends/`.
- e4mc reference: https://github.com/vgskye/e4mc-minecraft-architectury
  (MIT, Copyright (c) 2024 Skye); e4mc Quiclime Rust relay:
  https://github.com/vgskye/e4mc-quiclime (MIT OR Apache-2.0).
- Native Iroh: https://github.com/n0-computer/iroh
  (MIT OR Apache-2.0, additional BSD-3-Clause notices for incorporated
  Tailscale code). Verify exact dependency notices before shipping.
- No e4mc source is copied or translated in NET-01; this stage reuses the
  pre-existing Horizon TCP relay and adds original Rust relay failover.
- Source H001 vs fork H002 mirror equality: both 4680a73f at session start.
  Accepted H004 review remained in progress and was not changed by this branch.

## Acceptance / recovery

A NET-01 implementation is accepted only after local Rust checks pass and a
scoped commit preserves it on the topic branch. It is not upstream-merged nor
part of H004 until the separate guarded review and relevant multiplayer parity
checks are complete. Rollback by omitting the topic branch; existing worktrees
and upstream tracking refs must remain untouched.
