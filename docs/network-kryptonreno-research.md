# KryptonReno networking review for Horizon 26.3

Reviewed 2026-10-23 from the public `404Setup/KryptonReno` repository, default
branch `26.2.0`: https://github.com/404Setup/KryptonReno

**Purpose:** preserve actionable research while NET-02 Iroh / e4mc connectivity
remains higher priority. This is an implementation backlog, not competing
project management: canonical roadmap is the parent research workspace's
`reports/MASTER_ROADMAP.md`.

## License / provenance boundary

KryptonReno's repository `LICENSE` is LGPL-3.0-only. The README states a
supplemental restrictive 404Setup Public License and links to
https://github.com/404Setup/404Setup/blob/main/LICENSE.md (its terms restrict
binary redistribution, among other things). The combination warrants separate
legal review. **Do not copy or directly translate this repository's code or
benchmarks into Horizon's MIT crates under assumptions made for e4mc MIT.**
Instead independently implement general algorithms and standards using official
specifications, and retain clear provenance. RecastLib and SpringLotus have
separate license conditions; neither is vendored here. No KryptonReno source has
been added to Horizon in this topic branch.

## Source-grounded candidate techniques (do after Iroh integration)

| ID | KryptonReno reference | Horizon source | Decision / acceptance evidence |
| --- | --- | --- | --- |
| KR-01 | `Varint21FrameDecoderMixin` and `VarIntUtil` | `crates/horizon-protocol/src/frame.rs::split`, `buf.rs::Reader::varint` | Benchmark malformed/partial-frame handling, buffered reads and frame-copy cost. Horizon currently clones frame contents then `Vec::drain`s the input prefix, potentially moving the remaining bytes. Prototype independent cursor-based framing with bounded buffers. Require byte-for-byte parity, malformed-length, fragmented VarInt, zero-length and oversized tests; profile realistic packet mixes. |
| KR-02 | `MinecraftVarintPrepender`, compression pipeline | `crates/horizon-protocol/src/frame.rs::{prefixed,contents,decompress}` | Evaluate combined writes and fewer intermediate allocations only after profiling. Preserve compressed-threshold boundary and all 26.3 packets. Run identical payload categories and baseline/post RSS/allocations. |
| KR-03 | `HEConnect` RFC 8305 race | `crates/horizon-friends/src/direct.rs`, `crates/horizon-protocol/src/connection.rs` | Independently implement dual-stack latency-aware Happy Eyeballs where DNS names are resolved (not for endpoint IDs). Avoid simultaneous unfair connection spikes; use staggered starts, cancellation, endpoint filtering and timeout. The source reference marks the Java implementation `@Deprecated`; use RFC 8305 directly. |
| KR-04 | `PacketProcessorMixin` queue-drain changes | `crates/horizon-client/src/lan/conn.rs`, networking queues | Measure wakeups, lock contention, packet ingestion per tick first. Avoid concurrency reorder that would break existing Java 26.3 real-server packet-order gates. |
| KR-05 | Entity/particle culling mixins | Multiplayer visibility code | Deferred: can change gameplay observations and command/entity parity. Implement only against controlled Java oracle and complete entity tracking tests, not as a transport optimization. |
| KR-06 | JNI/FFM RecastLib encryption/compression | Horizon is native Rust already | Don't port Java-native adapters. Compare the native Rust codec's actual zlib/AES performance against known safe implementations first; don't assume native is automatically faster. |

Primary source navigation:

- `common/src/main/java/one/pkg/kreno/mixin/network/pipeline/Varint21FrameDecoderMixin.java`
- `common/src/main/java/one/pkg/kreno/shared/network/util/VarIntUtil.java`
- `common/src/main/java/one/pkg/kreno/shared/network/pipeline/MinecraftVarintPrepender.java`
- `common/src/main/java/one/pkg/kreno/shared/network/he/HEConnect.java`
- `common/src/main/java/one/pkg/kreno/mixin/network/pipeline/encryption/ConnectionMixin.java`
- `Benchmark.md` (authors explicitly warn differences near ~3% can be noise).

## Performance research discipline

1. Record benchmark environment (CPU, affinity, OS, compiler flags, memory,
   payload distributions, cold/warm and batch sizes) and a baseline before edits.
2. Reproduce packet fixtures against **original Java 26.3** where behavior could
   differ; fuzz truncated and malformed packets for bounds safety.
3. Compare throughput, p50/p95/p99, allocations and CPU, not latency alone.
   Preserve exact message ordering, compression semantics and encryption.
4. Adopt one targeted algorithm only if the improvement is independently
   measurable in Horizon without increasing its attack surface.

**Verified current Horizon baseline:** `frame.rs::split` builds a new `Vec` for
every complete frame and drains consumed bytes from its persistent buffer;
`contents()` builds a separate compressed body `Vec`, and `prefixed()` copies
that payload to another `Vec`. This is source observation, not a benchmark.
No KryptonReno-inspired runtime optimization has been implemented or measured
in this session; all KR tasks remain research/pending after native Iroh work.
