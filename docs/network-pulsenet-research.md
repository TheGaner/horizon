# PulseNet source review — applications to Horizon's Rust packet pipeline

**Status:** source-grounded research and prioritized test backlog; **no PulseNet code copied** and no runtime speedup claimed. Reviewed on 2026-10-23 from `Pulse-MC/PulseNet` at commit `a3e02d960dc56f788a32b377b7c1c59304fe54d8`, Minecraft 26.3. The comprehensive original analysis is **preserved without modification** at `/home/pc/Projects/horizon-parity-performance/pulsenet-documentation/` (master review plus eight reference chapters and generated inventory). Source clone: `/home/pc/Projects/pulsenet-review/PulseNet/`. Repo: https://github.com/Pulse-MC/PulseNet/tree/a3e02d960dc56f788a32b377b7c1c59304fe54d8.

**License boundary:** PulseNet `LICENSE.txt` states `Copyright (c) 2026 BorisShoes, Tizis, justc2t — All rights reserved`; `fabric.mod.json` agrees. A README describing it as open source is not a redistribution/porting license. **Do not copy, translate, adapt or vendor PulseNet's implementation** into Horizon under its existing MIT license without permission. The table below identifies standard algorithmic concepts and independently written alternatives, with source provenance for comparative research only. No unlicensed implementation is imported by this document.

## Important separation of networking layers

- **e4mc / Iroh (NET-02):** device-to-device transport establishment, UDP hole punching, QUIC/TLS identities, relay fallback, session privacy. The working Iroh module remains opt-in; the production route still uses Horizon's encrypted TCP friends tunnel.
- **KryptonReno (KR-01..KR-06):** VarInt framing, codec performance, allocation/copy reduction, dual-stack connection racing and network-thread behavior.
- **PulseNet (PN-01..PN-07):** outbound *server* packet scheduling, ordered flushing, optional bundle/chunk substitution and metrics; it does not implement P2P NAT traversal or guarantee bandwidth savings.

## What Horizon already has — do not reimplement

Source inspected in this topic branch:

- `crates/horizon-client/src/lan/conn.rs::Holding`, `PlayerSender::{send,send_many,flush,disconnect}`, `flush_all`, `post`: per-player ordered held tick batches and a post thread that hands a batch to each player writer in order, rather than one wake-up per packet. `packets_sent_after_a_posted_tick_follow_it` already tests packet-before/after-flush and disconnect ordering. Avoid PulseNet's three independent class-based queues, which can reorder network events.
- `lan/conn.rs::write_loop` uses one writer per connection and a `pending` atomic with an existing `MAX_PENDING` cutoff; pending estimates count raw packet body bytes, **not verified outstanding serialized or kernel bytes**. Audit error propagation and backpressure before replacing it.
- `lan/conn.rs::SharedFrames` has a size-bounded cache of compressed packet contents across clients (`SHARED_FRAME_BYTES`); independently check CPU, hits, collisions, contention and cache residency before copying any caching approach.
- `lan/hub.rs::next_batch` / `update_chunks` already pace chunk batches using client feedback; `broadcast_block_changes` groups changes into `BTreeMap` sections and preserves block-entity update emission. Never replace dense changes with a snapshot unless Java26.3 oracle proves equivalence.
- `crates/horizon-protocol/src/{frame,connection}.rs` provide VarInt21 framing, zlib and a single `ConnWriter::send`; optimize allocations jointly with KR-01/KR-02 rather than introducing competing batching layers.

## Prioritized independently implementable ideas

| ID | Reference finding or goal | Horizon target / next action | Guard / evidence required |
| --- | --- | --- | --- |
| **PN-01 (P0)** | PulseNet F01/F02: fixed drain order across block, coalesce and write queues lets newer packets overtake older ones | Preserve existing single ordered `PlayerSender` stream. Expand `lan/conn.rs` ordering tests to concurrent held flush, critical disconnect, chat, chunk, custom payload and configuration transitions, including failure/close | Original Java26.3 trace-parity where semantics matter; no reordered dependent packets; exhaustive multi-producer test |
| **PN-02 (P1)** | PulseNet F03/F20: unbounded/backlog counters omit some queues and bytes | Audit `pending`, `posted`, `held` and shared cache under slow/stalled clients; cap held **bytes and packets** before writer thread and bounded post backlog, after measuring current behavior | Bytes in memory versus bodies tracked, high-water mark, fairness, disconnect/cancellation and no silent loss |
| **PN-03 (P1)** | PulseNet F09/F16/F23: flush scheduling ≠ completed writes or syscalls | Add scoped instrumentation for enqueue, batching handoff, write completion/error, bytes encoded and socket write calls; distinguish *application writes* from real kernel/network packets | Compare counters to actual write operations under backpressure; keep overhead opt-in; p50/p95/p99 latency |
| **PN-04 (P2)** | PulseNet F04/F05/F08: wrong-thread processing and drain/reset races | Keep `World`/chunk construction on game thread; test post-thread ordering and change-of-tick boundaries under concurrent senders, stop, reload | No off-thread world mutation; Loom-equivalent controlled race tests where practical |
| **PN-05 (P2)** | PulseNet F06/F07/F14/F15: bundle/chunk replacement may omit callbacks or change protocol semantics | *Do not enable* new bundle or full-chunk replacement yet; first compare Horizon's existing `lan/hub.rs::broadcast_block_changes` with original Java26.3 snapshots and staged block entity updates | Exact client final state, block entities, session lifecycle and ordered packet trace across dimensions; original callbacks if API has them |
| **PN-06 (P2)** | PulseNet F10/F19: configurable priority tables could demote critical packets, and hot reload can mix generations | If adding packet categories, define non-overridable protocol-critical boundaries and atomic immutable config; audit login/configuration/play phases | Keepalive/disconnect/registration always ordered; reload race and malformed-config negative tests |
| **PN-07 (P3)** | PulseNet F11/F21/F22: flush-mode semantics and misparsed settings | Any new toggle must have tested effects, bounded inputs and safe migration; resist exposing fake `strict_tick` modes | Config round-trip and full control-plane/UI integration tests before shipping |

## Comparative findings with KryptonReno

- **KR-01/02 + PN-03:** Profile `frame::split` copy/drain, `frame::prefixed`, `frame::contents` and `SharedFrames` cache together; reducing a copy matters only if it improves measured end-to-end throughput/latency without regressions.
- **KR-04 + PN-01/02/04:** Study Rust's existing `PlayerSender` and writer queue for contention/fairness first. Keep one ordered logical stream: priority classes may influence the decision to flush, but must not move a later packet ahead of an earlier protocol dependency.
- **KR-05 + PN-05:** Entity/particle culling, bundling and full-chunk replacement are **semantic** transformations, not pure transport speedups. Defer until strict Java26.3 parity, with dimensions, unload/reload and block-entity cases.
- **NET-02 + PN-02/03:** QUIC and TCP require separate transport-level backpressure and RTT reporting. Batch Minecraft messages based on actual flow control; do not presume fewer app writes means fewer wire packets.

## Validation evidence and limitations

PulseNet's own reference workspace reports a successful **Java 25 Gradle build**, but no upstream automated tests and no live-server throughput or compatibility verification. Its 25 correctness notes mix source-confirmed control flow with concurrency hypotheses; **do not describe all as reproduced bugs**. Specifically, the important confirmed behavior is three separate queues drained in priority order, while whether a player actually observes a gameplay regression must be reproduced. Source: `pulsenet-documentation/docs/01_...`, `04_...`, `05_...`, and `06_...` (see the original directory for full filenames).

**Recommended benchmark first:** controlled tick-burst and slow-reader baselines at 1, 4, 16 and 64 clients; collect CPU cycles, allocations, buffer sizes, connection write counts, packet ordering and p50/p95/p99 latency. Compare controlled workload traces from the original Minecraft Java26.3 oracle before any game-visible transformations. Keep the author's original full report as the reference; this document is an actionable crosswalk, not a substitute for its high-fidelity analysis.
