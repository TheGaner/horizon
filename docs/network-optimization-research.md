# Horizon network improvement references — unified research index

**Purpose:** Keep the existing e4mc/Iroh transport work as the immediate implementation priority while grouping two independently researched packet-pipeline projects, KryptonReno and PulseNet, into one actionable, nonduplicated Rust optimization backlog. No optimization is claimed without measurements and Java26.3 behavioral equivalence. Canonical milestones remain in `/home/pc/Projects/horizon-parity-performance/reports/MASTER_ROADMAP.md`.

| Layer | Reference / tracking document | Next priority | Current state |
| --- | --- | --- | --- |
| Network connectivity | [Network transport modernization](network-transport-modernization.md) — e4mc concepts, native Iroh | NET-02C/02D connection policy and real network traversal | Optional Iroh QUIC + V3 invite codec implemented and tested on localhost; gameplay integration pending |
| Packet framing / codecs | [KryptonReno research](network-kryptonreno-research.md) | KR-01/02 VarInt and compression allocation baselines, later KR-03/04 scheduling | Source review documented; independent implementation pending |
| Server outbound pipeline | [PulseNet research](network-pulsenet-research.md) | PN-01 order invariants / PN-02 backpressure / PN-03 real measurements | Source research documented; Horizon already has ordered packet batching; no new optimization copied |

## Complete offline reference library

- **PulseNet full source review and generated inventory:** `/home/pc/Projects/horizon-parity-performance/pulsenet-documentation/` — approximately 350 KB of retained documentation, with `PULSENET_COMPLETE_TECHNICAL_REVIEW.md`, `docs/01..08`, `docs/tools` and a symlink to the separately cloned exact upstream source `a3e02d960d...`. These files were **not relocated, duplicated or edited**.
- **KryptonReno original source:** https://github.com/404Setup/KryptonReno. See `network-kryptonreno-research.md` for exact classes and license cautions.
- **e4mc:** https://github.com/vgskye/e4mc-minecraft-architectury; **Iroh:** https://github.com/n0-computer/iroh; keep separate code/infrastructure licensing and endpoint-identity security reviews.

## Engineering decisions

1. **Avoid duplication:** Horizon's `lan/conn.rs` already has per-player ordered tick batching, a post-thread handoff, per-connection writer and shared frame-compression cache. Benchmark them before introducing another queue or coordinator.
2. **Ordering and correctness before latency:** Single ordered stream, strict protocol barriers, honest completions, bounded memory, thread-confinement and multi-client fairness are prerequisites for optional batching/coalescing. Preserve Java26.3 packet parity.
3. **Licensing:** KryptonReno repository indicates LGPL-3.0-only and additional restrictions; PulseNet explicitly reserves all rights. Reference ideas/specifications, write original Rust and seek rights clearance before translating, vendoring or distributing derivative source.
4. **Implementation sequence:** Complete NET-02 e4mc/Iroh compatibility; take network performance baselines; verify PN-01 and PN-02 safety; evaluate KR-01/02 frame/codec work; benchmark PN-03 measurements and later semantic optimizations as individually gated changes.

No public relay or real CGNAT capability is implied by this index. Research does not mean shipped code.
