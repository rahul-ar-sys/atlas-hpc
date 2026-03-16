Here is the markdown instruction file tailored for your coding agent, Antigravity, to build the Atlas-Finance MVP. It strips away the product fluff and focuses strictly on the engineering constraints and architectural mandates from the PRD and TRD.

---

# Antigravity Instructions: Atlas-Finance MVP Initialization

**Context:** You are tasked with building the MVP for Atlas-Finance (Codename: "HPC-Financial-Brain"), an agent-centric financial intelligence platform designed to disrupt institutional terminals. The core architectural philosophy is to treat latency as a memory problem.

**Strict Engineering Directives:** * Prioritize deterministic, High-Frequency Trading (HFT) speed reasoning.

* Reject standard RAG patterns; implement Weighted Cache Reuse and Causal Graphs.


* Ensure zero lock-contention during 1M ops/sec bursts.



## 1. Technical Stack & Environment Initialization

* 
**Language:** Initialize project in Rust 1.85+ (enable Nightly features for SIMD).


* 
**Concurrency:** Import `crossbeam` for epoch-based reclamation and `leapfrog` for high-contention hash maps.


* 
**Networking:** Set up DPDK (Data Plane Development Kit) or SPDK for kernel bypass to handle market feeds directly.


* 
**Graph Engine:** Provision Neo4j 5.25+ or FalkorDB and integrate Graphiti.



## 2. Phase 1: Implement the "Hot Tier" (KV Store)

**Objective:** Deliver a Rust-based KV store achieving `< 5 microseconds` read latency and `< 10 microseconds` write latency.

* 
**Data Structure:** Implement the `WeightedEntity` struct exactly as specified with 64-byte cache line alignment:


```rust
#[repr(C, align(64))]
pub struct WeightedEntity {
    pub entity_id: u64,
    pub latest_price: f64,
    pub volatility_score: f32,
    pub importance_weight: f32,
    pub causal_trigger_bit: bool,
    pub last_updated_ns: u64,
}

```


* 
**Weight Recalculation:** Implement a zero-copy path to update importance weights. Use the following formula where $S$ is the surprise factor and $\alpha, \beta$ are decay/sensitivity constants:



$$W_{new} = (W_{old} \times \alpha) + (S \times \beta)$$


* 
**Trigger Logic:** The Sensing Agent must continuously poll this store. If `causal_trigger_bit == true`, it must immediately dispatch a `SignalEvent` to the Reasoning Agent.



## 3. Phase 2: Implement the "Warm Tier" (Causal Graph)

**Objective:** Build a linked-list based storage system for semantic labels with spreading activation traversal under 100 microseconds.

* 
**Labeling Engine:** Build an ingestor that tags incoming SEC filings and news API data with specific semantic `LabelID`s (e.g., `L-CRUDE-SHOCK`).


* 
**Virtual Edge Creation:** Program the logic to materialize a `TEMPORAL_LINK` edge when the Sensing Agent observes a weight spike in an entity, queries the graph for shared labels, and confirms historical correlation (p-value < 0.05).


* 
**Accuracy Target:** Graph traversal must maintain a >99.5% grounding (Fact-to-Hallucination ratio) using historical ticker/SEC data.



## 4. Phase 3: Intelligence & Cache Orchestration

**Objective:** Implement multi-agent reasoning utilizing sub-second cache reuse.

* 
**LMCache Integration:** Implement position-independent prefix caching to bypass the context prefill penalty for static financial documents.


* 
**Invalidation Logic:** Implement "Dirty-Bit Invalidation". When a graph node updates, clear *only* the reasoning paths dependent on that specific node to preserve the remaining context.


* 
**Memory Management:** Configure dynamic offloading to move idling agent states to NVMe via SPDK, reserving GPU VRAM for HFT sensing.



## 5. Non-Functional Enforcement (The "Brutal" Metrics)

* **Jitter:** Enforce strict latency bounds; P99.99 must remain under 50 microseconds.


* 
**Hardware Pinning:** Configure core pinning for the Hot Tier to Cores 0-3.


* 
**Security:** Wrap all firm-proprietary weights and graph edges in hardware-level Trusted Execution Environments (Intel TDX or AMD SEV-SNP).



---

Would you like me to draft the initial Rust boilerplate for the `WeightedEntity` struct and the lock-free DPDK memory mapping to kickstart Antigravity's Phase 1 execution?