# Atlas-Finance: HPC-Financial-Brain

> **Codename:** HPC-Financial-Brain  
> **Language:** Rust (Nightly) 1.85+  
> **Philosophy:** Treat latency as a memory problem.

Atlas-Finance is an experimental, agent-centric financial intelligence platform. It is designed to bridge the gap between **High-Frequency Trading (HFT) data ingest** and **Agentic Large Language Model (LLM) reasoning**, achieving sub-millisecond semantic analysis with zero lock-contention at >1M ops/sec.

---

## 1. The Novelty: Colliding HFT with Generative AI

Historically, financial architecture exists in two isolated silos:
1. **The Quant Silo:** Top-tier prop shops use C++, Rust, and FPGAs to process market data in single-digit microseconds. However, this infrastructure only executes static mathematical models; it cannot reason about unstructured data (e.g., semantic shifts in SEC filings or Fed press conferences).
2. **The AI Silo:** Generative AI and standard RAG (Retrieval-Augmented Generation) architectures use Python-based vector databases. They are mathematically capable of semantic reasoning but measure latency in hundreds of milliseconds or full seconds. They are entirely decoupled from microsecond-level tick data.

**The Atlas-Finance Thesis:** We treat LLM latency strictly as a memory orchestration problem. Instead of a standard RAG pipeline that searches a database *after* an event occurs, Atlas-Finance utilizes a deterministic HFT pipeline to mathematically trigger a non-deterministic AI swarm. 

We pre-load static reasoning context (e.g., Fed Policy frameworks) into VRAM and use microsecond-level "Surprise Volatility" algorithms to act as a wake-up call for semantic reasoning.

---

## 2. Technical Architecture Deep Dive

The system is separated into three memory-bound tiers, strictly enforcing lock-free concurrency and CPU-core pinning.

```text
  Live Market UDP / WebSocket Stream
                 │
                 ▼
┌───────────────────────────────────────┐
│  Phase 1 — Hot Tier  (KV Store)       │
│  Lock-Free DashMap / LeapMap          │
│  < 5 µs reads / < 10 µs writes        │
└───────────────┬───────────────────────┘
                │ SignalEvent (crossbeam channel)
                ▼
┌───────────────────────────────────────────────┐
│  Phase 2 — Warm Tier  (Causal Graph)          │
│  In-Memory Arena-Allocated Graph              │
│  TEMPORAL_LINK materialisation (p < 0.05)     │
│  < 100 µs spreading activation                │
└───────────────┬───────────────────────────────┘
                │
                ▼
┌──────────────────────────────────────────────────────┐
│  Phase 3 — Intelligence Layer                        │
│  ReasoningAgent + Local LLM (Ollama/Llama-3)         │
│  PrefixCache (position-independent context assembly) │
│  Dirty-Bit Invalidation & NVMe offloading            │
└──────────────────────────────────────────────────────┘

```

### Tier 1: The Hot Tier (KV Store)

The pulse of the system. Designed to ingest millions of market ticks per second.

* **Data Locality:** Entities are tracked using the `WeightedEntity` struct, strictly aligned to a 64-byte cache line (`#[repr(C, align(64))]`) to prevent false-sharing across threads.
* **Surprise Volatility:** Weights are dynamically recalculated on every tick: `W_new = (W_old * α) + (Surprise * β)`.
* **The Trigger:** If the $\Delta W$ breaches a hardcoded threshold, a `causal_trigger_bit` is flipped, and the core-pinned `SensingAgent` instantly dispatches a lock-free `SignalEvent` to the Intelligence Tier.

### Tier 2: The Warm Tier (Causal Graph)

Replaces traditional vector search with an in-memory Semantic Graph.

* **Semantic Labeling:** Financial entities are tagged with overlapping semantic boundaries (e.g., `L_CRUDE_SHOCK`, `L_RATE_HIKE`).
* **Deterministic Edge Materialization:** A directed `TemporalLink` is only formed between nodes if historical return samples yield a Pearson correlation p-value of `< 0.05`.
* **Traversal:** Utilizing a Breadth-First Search (BFS) spreading activation algorithm, the system can traverse 10,000 entities in `< 100 microseconds` to assemble multi-hop market context.

### Tier 3: The Intelligence Tier

Orchestrates the LLM reasoning context securely and rapidly.

* **Prefix Caching:** Bypasses the LLM context prefill penalty. Static policy documents are stored as pre-computed KV caches.
* **Dirty-Bit Invalidation:** When a Warm Tier graph node updates, the system calculates set intersections and flags only the specific dependent reasoning paths as `dirty = true`, preserving the rest of the VRAM cache.

---

## 3. Key Data Structures

| Type | Description |
| --- | --- |
| `WeightedEntity` | 64-byte cache-line-aligned entity record (`#[repr(C, align(64))]`). |
| `SignalEvent` | Enum: `WeightSpike` / `CausalTrigger` dispatched by SensingAgent. |
| `LabelId` | Semantic label (`L_CRUDE_SHOCK`, `L_RATE_HIKE`, etc.). |
| `TemporalLink` | Directed causal edge requiring Pearson p-value < 0.05. |
| `PrefixCache` | DashMap-backed LMCache-style prefix cache with dirty-bit invalidation. |
| `SoftwareTeeGuard` | AES-256-GCM stub for hardware memory enclaves. |

---

## 4. Performance Targets & Metrics

These metrics are validated via standard Rust benchmarking (`Criterion`) and high-concurrency HTTP load testing (`oha` / `wrk2`).

| Metric | Target | Verification Method |
| --- | --- | --- |
| Hot Tier read P50 | `< 5 µs` | `cargo bench -p hot-tier` |
| Hot Tier write P50 | `< 10 µs` | `cargo bench -p hot-tier` |
| BFS traversal P99 | `< 100 µs` | `cargo bench -p warm-tier` |
| Concurrency | `> 100k RPS` | `oha -c 100 -z 10s http://127.0.0.1:3000/api/v1/alpha/42` |
| Grounding ratio | `> 99.5 %` | `assert!(graph.grounding_ratio() >= 0.995)` |

---

## 5. Getting Started

### Prerequisites

* Rust **nightly** (pinned via `rust-toolchain.toml`).
* Windows/Linux/macOS environment (Linux highly recommended for core pinning).

```bash
# Build the workspace
cargo build --workspace --release

# Run the localized nanosecond benchmarks
cargo bench -p hot-tier
cargo bench -p warm-tier

# Run the live simulation (Synthetic HFT Feed + Axum API Server)
cargo run --release

```

### Running with Local LLM Reasoning

To test the full Phase 3 pipeline, install [Ollama](https://ollama.com/) and pull a lightweight reasoning model:

```bash
ollama pull llama3

```

Ensure the Ollama server is running on `localhost:11434`. The `ReasoningAgent` will automatically detect the endpoint and begin generating "Explainable Alpha" context based on the HFT triggers.

---

## 6. Future Scope & Commercial Alpha

Transitioning this MVP into a production-grade commercial platform requires solving several brutal hardware and physics limitations:

1. **The Kernel-Bypass Integration:**
* **Current State:** The MVP uses standard OS TCP/WebSocket stacks (via `tokio-tungstenite`) and synthetic deterministic feeds.
* **Future:** Implement exact C/FFI bindings for **DPDK (Data Plane Development Kit)** or **SPDK**. We must utilize `rte_ring` memory mapping to bypass the Linux kernel entirely and read UDP multicast market data directly into the user-space `HotStore`.


2. **The TEE Latency Wall:**
* **Current State:** Multi-tenant security is stubbed via a `SoftwareTeeGuard` using AES-256-GCM.
* **Future:** Integrate **Intel TDX** or **AMD SEV-SNP** hardware enclaves to ensure proprietary firm strategies do not leak into the global graph. *Challenge:* Hardware enclaves incur heavy context-switching latency that actively antagonizes the strict `< 50 µs` jitter requirement.


3. **The Hallucination Financial Risk:**
* While the Warm Tier enforces a strict statistical grounding ratio (`p < 0.05`), allowing an autonomous LLM swarm to execute live trades based on semantic interpretation remains a massive compliance risk. The immediate commercial scope is restricted to an **"Explainable Alpha UI"**—a read-only terminal overlay that feeds reasoning data to human Portfolio Managers, rather than executing autonomous writes to a brokerage API.



```

```