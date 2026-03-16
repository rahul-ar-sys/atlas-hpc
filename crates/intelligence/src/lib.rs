//! # intelligence
//!
//! Phase 3: Agent orchestration, prefix-cache, dirty-bit invalidation, TEE stub,
//! and NVMe offload interface.

#![deny(missing_docs)]

pub mod cache;
pub mod orchestrator;
pub mod tee;
pub mod offload;
pub mod agent;
