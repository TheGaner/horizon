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
explicitly trusted matching pool. The active relay is included in
`HostState` and visible on the sharing screen for troubleshooting; no invite
secret is logged. Legacy single-address behavior remains.
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

**Dependency gate: independently resolved for experimental feature.** Horizon
normally pins `rustc 1.90.0` via `../scripts/horizon_env.sh`; native Iroh 1.4.1
requires Rust >=1.91. Rust 1.91.0 was installed **alongside**, not instead of,
1.90 in the shared rustup install. Run only the experimental feature with an
explicit `RUSTUP_TOOLCHAIN=1.91.0` override. The default gameplay builds must
continue to compile on the pinned Rust 1.90, and the production workspace MSRV
is unchanged. Production release/CI toolchain migration requires a separate
review and full workspace gate.

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

## NET-02A — opt-in QUIC localhost proof

**Implementation in this topic branch (experimental, not selectable in-game):**
`crates/horizon-friends/src/iroh_transport.rs` uses native Rust Iroh 1.4.1,
Tokio and QUIC streams behind Cargo feature `iroh-experimental`. It binds an
endpoint on local UDP, provides an `EndpointAddr` and 32-byte capability ticket,
authenticates the capability **before** dialing the integrated TCP loopback
server, and pipes Minecraft TCP bytes bidirectionally between QUIC and TCP.
A guest can expose the QUIC stream as a local `127.0.0.1` TCP port, matching
Horizon’s existing remote client contract. All proof code is written in Rust.

Iroh `presets::Minimal` disables public DNS lookups and relays: these tests
exercise encrypted QUIC **over loopback UDP only**. It must NOT be described as
Internet NAT traversal, regional relay discovery or a replacement of the
production encrypted TCP invite protocol. The new ticket is not yet encoded
in legacy invite v2, and there is not yet an automatic path selector, public
service or in-game opt-in. Production must maintain relay-only IP privacy;
no UDP probe should run without explicit user consent in that mode. QUIC's TLS
endpoint identity protects against endpoint impersonation while the separate
capability authenticates authorization. This prototype's bearer capability
is NOT equivalent to the existing SPAKE2 negotiation and requires a security
review, replay limits and secret/identity binding before production use.

The tests `iroh_transport::tests` cover successful TCP echo over QUIC and
rejecting a wrong capability before the local server is reached. No public
service is contacted by design. Timeout and host shutdown semantics need
focused testing beyond these two cases. Reproduce from the checkout root:

```sh
. ../scripts/horizon_env.sh
RUSTUP_TOOLCHAIN=1.91.0 cargo test --locked -p horizon-friends --features iroh-experimental --lib iroh_transport
RUSTUP_TOOLCHAIN=1.90.0 cargo test --locked -p horizon-friends --lib
```

**Next engineering slice:** Add a versioned invitation encoding that binds the
Iroh EndpointId and capability securely and is backward-compatible with v2;
apply `PathMode` direct-only/relay-only policies to real Iroh UDP transport;
wire the adapter into the host/guest lifecycle. Add per-client concurrency
budgets, slowloris/replay tests, multi-guest sessions, and a real CGNAT / relay
harness before selecting QUIC by default.

## NET-02B — experimental versioned `friendq:` invitations

**Implemented in this topic branch, but still feature-gated and not usable from
Horizon gameplay UI:** `crates/horizon-friends/src/iroh_invite.rs` adds a strict
Rust encoder/decoder for a separate `friendq:` prefix. The legacy `friend:`
v2 TCP + SPAKE2 code, parser and gameplay networking call graph are unchanged.

The experimental V3 binary payload is `HIFQ` magic (4 bytes), version `3`
(1), Iroh authenticated endpoint public key (32), randomly generated capability
from `Host::bind_local_random` (32), address count (1, max 8), tagged direct
IPv4/IPv6 sockets (tag+IP+big-endian port), one optional HTTPS-compatible
Iroh relay URL (length byte, max 255), and an eight-byte SHA-256 checksum
prefix for accidental corruption **only**. Crockford Base32 groups of five
follow existing share-code conventions. Decoder rejects malformed/invalid
Iroh keys, zero ports, duplicate sockets, oversized text/bytes, invalid tags,
unsupported versions, trailing binary data, overlong fields, noncanonical
representations and redundant padding bits. The new token is a *bearer
credential* and must never be logged or sent as telemetry. The checksum is
**not a signature**, and does not protect an invite from intentional edits.

`iroh_transport::tests::authenticated_quic_carries_minecraft_tcp_without_public_services`
now transports actual Minecraft TCP bytes over a localhost Iroh QUIC session
**using a decoded V3 invitation**, not just an in-memory ticket. Dedicated
codec tests cover IPv4, IPv6, relay URLs, legacy-v2 separation, corruption,
duplicate/invalid endpoints, size limits and noncanonical representations.
Running all tests with `RUSTUP_TOOLCHAIN=1.91.0 cargo test --locked -p
horizon-friends --features iroh-experimental --lib` exercises both existing
friends/relay logic and the new codec. This is a localhost proof only; Iroh
relay discovery, real NAT traversal, PathMode-specific builders, multi-guest
acceptance, replay controls, connection UI and production security review are
**not implemented**. An encoded relay URL does not configure a relay, nor
imply authorization to use a third party's relay service.

The design contract in
`/home/pc/Projects/horizon-parity-performance/reports/networking/NET-02B-IROH-INVITE-AND-SECURITY-CONTRACT-2026-10-23.md`
is the basis for continued hardening. Its provisional format sketch differs
from the implemented V3 payload (no flag byte yet); this section and the
source module define the **implemented experimental format**. No production
commit should rely on the experimental format without compatibility review.

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

## Additional performance research

KryptonReno pipeline, VarInt, compression and Happy Eyeballs ideas are tracked
separately in [`network-kryptonreno-research.md`](network-kryptonreno-research.md)
for follow-on work **after** the native Iroh connection path. These optimizations
need identical packet-contract tests and A/B profiling; they do not replace
NAT traversal or offer free access to e4mc public relays.


## Combined networking reference library

The [unified optimization research index](network-optimization-research.md) groups the native-Iroh/e4mc connectivity priorities with the [KryptonReno](network-kryptonreno-research.md) and [PulseNet](network-pulsenet-research.md) packet-processing findings. They are independent layers: PulseNet-style packet batching is **not** a substitute for UDP NAT traversal, and Horizon already has an ordered tick-batching implementation. Preserve the original documents and license restrictions; use controlled performance and Java26.3 parity tests before adopting a new packet-level optimization.
