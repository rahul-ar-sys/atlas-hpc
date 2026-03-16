# Atlas-Finance HPC-Financial-Brain MVP

> **Codename:** HPC-Financial-Brain  
> **Language:** Rust (Nightly)  
> **Philosophy:** Treat latency as a memory problem.

An agent-centric financial intelligence platform targeting HFT-grade, sub-millisecond reasoning with zero lock-contention at 1M ops/sec.

---

## Architecture

```
Binance WebSocket (!ticker@arr)
        │
        ▼
┌───────────────────────────────────────┐
│  Phase 1 — Hot Tier  (hot-tier crate) │
│  DashMap<u64, WeightedEntity>         │
│  < 5 µs reads / < 10 µs writes        │
└───────────────┬───────────────────────┘
                │ SignalEvent channel (crossbeam)
                ▼
┌───────────────────────────────────────────────┐
│  Phase 2 — Warm Tier  (warm-tier crate)       │
│  CausalGraph: label store + BFS traversal     │
│  TEMPORAL_LINK materialisation (p < 0.05)     │
│  < 100 µs spreading activation                │
└───────────────┬───────────────────────────────┘
                │
                ▼
┌──────────────────────────────────────────────────────┐
│  Phase 3 — Intelligence  (intelligence crate)        │
│  ReasoningAgent + Local LLM (Ollama/Llama-3)         │
│  PrefixCache (pos-independent context assembly)      │
│  Dirty-Bit Invalidation / NVMe offload / TEE guard   │
└──────────────────────────────────────────────────────┘
```

## Key Data Structures

| Type | Description |
|------|-------------|
| `WeightedEntity` | 64-byte cache-line-aligned entity record (`#[repr(C, align(64))]`) |
| `SignalEvent` | Enum: `WeightSpike` / `CausalTrigger` dispatched by SensingAgent |
| `LabelId` | Semantic label (`L_CRUDE_SHOCK`, `L_RATE_HIKE`, etc.) |
| `TemporalLink` | Directed causal edge with Pearson p-value < 0.05 |
| `PrefixCache` | DashMap-backed LMCache-style prefix cache with dirty-bit invalidation |
| `SoftwareTeeGuard` | AES-256-GCM TEE stub (Intel TDX / AMD SEV-SNP interface) |

## Workspace Layout

```
atlas-mvp/
├── src/main.rs                  ← Integration harness & simulation
├── crates/
│   ├── atlas-types/             ← Shared types (WeightedEntity, SignalEvent, ...)
│   ├── hot-tier/                ← Phase 1: Lock-free KV store + SensingAgent
│   ├── warm-tier/               ← Phase 2: CausalGraph + BFS traversal
│   └── intelligence/            ← Phase 3: ReasoningAgent, Cache, TEE, NVMe
├── rust-toolchain.toml          ← Pins nightly channel
└── .cargo/config.toml           ← target-cpu=native, LTO flags
```

## Building

Requires Rust **nightly** (pinned via `rust-toolchain.toml`).

```bash
# Install Rust nightly (if not already installed)
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh
rustup toolchain install nightly

# Build all crates
cargo build --workspace --release

# Format code
cargo fmt --all

# Run all tests
cargo test --workspace

# Run the live integration simulation (Binance WebSocket + LLM Reasoning)
cargo run --release

# Run benchmarks (requires release, outputs HTML to target/criterion/)
cargo bench --workspace
```

## Running with Local LLM Reasoning

The `ReasoningAgent` detects causal spikes and synthesizes insights. By default, it POSTs prompts to a local LLM endpoint conforming to the OpenAI REST structure at `http://localhost:11434/v1/chat/completions`.
1. Install [Ollama](https://ollama.com/)
2. Pull a performant reasoning model (e.g., `ollama pull llama3`)
3. Ensure the Ollama server is running locally on port 11434 before running `cargo run --release`.

> **Note:** If Ollama is offline or unreachable, the Intelligence engine gracefully falls back to generating deterministic raw metric summaries without crashing, allowing core HFT data pipelines to continue uninterrupted.

## Non-Functional Targets

| Metric | Target | How Verified |
|--------|--------|-------------|
| Hot Tier read P50 | < 5 µs | `cargo bench hot-tier` |
| Hot Tier write P50 | < 10 µs | `cargo bench hot-tier` |
| BFS traversal P99 | < 100 µs | `cargo bench warm-tier` |
| P99.99 jitter | < 50 µs | `cargo bench` + `perf stat` |
| Grounding ratio | > 99.5 % | `assert!(graph.grounding_ratio() >= 0.995)` |
| Causal trigger dispatch | ≤ 1 poll sweep | Unit test in hot-tier |

## CPU Core Pinning (Linux Only)

The `SensingAgent` attempts to pin to core 0 at startup via `core_affinity`.  
The Hot Tier thread pool should be deployed on cores 0–3 using `taskset` or `numactl`:

```bash
numactl --cpubind=0-3 --membind=0 ./target/release/atlas-mvp
```

## TEE & Security

Sensitive weights and graph edges are wrapped in `SoftwareTeeGuard` (AES-256-GCM).  
In production, swap `SoftwareTeeGuard` for an Intel TDX or AMD SEV-SNP implementation  
conforming to the `seal() / unseal()` interface in `crates/intelligence/src/tee.rs`.

## DPDK / Kernel-bypass

The `MarketFeedAdapter` trait in `crates/hot-tier/src/lib.rs` decouples the feed source.  
`SoftwareFeedAdapter` is used on Windows / dev machines.  
On Linux production, implement with `rte_ring` + hugepage DMA for true DPDK bypass.
