//! NVMe / SPDK offload stub.
//!
//! On Linux with SPDK this would use `spdk_nvme_ns_cmd_write` to DMA agent
//! state directly to NVMe, bypassing the kernel I/O stack.
//!
//! For the MVP and Windows dev environments, state is serialised to a temp
//! file, providing the same interface without the hardware dependency.

use std::path::PathBuf;
use serde::{de::DeserializeOwned, Serialize};
use tracing::debug;

// ─────────────────────────────────────────────────────────────────────────────
// Agent state wrapper
// ─────────────────────────────────────────────────────────────────────────────

/// Serialised snapshot of an idle agent's reasoning state.
#[derive(serde::Serialize, serde::Deserialize, Clone, Debug)]
pub struct AgentState {
    /// Which agent owns this state.
    pub agent_id: String,
    /// Opaque JSON-serialised reasoning context.
    pub context_blob: Vec<u8>,
    /// Timestamp of snapshot (ns since UNIX epoch).
    pub snapshotted_at_ns: u64,
}

// ─────────────────────────────────────────────────────────────────────────────
// NvmeOffloadAdapter trait
// ─────────────────────────────────────────────────────────────────────────────

/// Interface for NVMe offload operations.
///
/// Implement this trait with a real SPDK backend for production Linux.
pub trait NvmeOffloadAdapter: Send + Sync + 'static {
    /// Write `state` to persistent storage.
    fn offload(&self, state: &AgentState) -> Result<OffloadHandle, OffloadError>;
    /// Read back a previously offloaded state.
    fn restore(&self, handle: &OffloadHandle) -> Result<AgentState, OffloadError>;
    /// Remove a previously offloaded state.
    fn drop_offload(&self, handle: &OffloadHandle) -> Result<(), OffloadError>;
}

/// An opaque handle referencing an offloaded agent state blob.
#[derive(Clone, Debug)]
pub struct OffloadHandle {
    /// Unique ID for this offload slot.
    pub slot_id: u64,
}

/// Errors from the offload layer.
#[derive(Debug)]
pub enum OffloadError {
    /// I/O failure.
    Io(String),
    /// Serialisation failure.
    Serialization(String),
}

impl std::fmt::Display for OffloadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            OffloadError::Io(e) => write!(f, "OffloadError::Io: {e}"),
            OffloadError::Serialization(e) => write!(f, "OffloadError::Serialization: {e}"),
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Software (file-backed) offload adapter
// ─────────────────────────────────────────────────────────────────────────────

/// File-backed NVMe offload adapter for development.
///
/// Writes serialised `AgentState` blobs to `<dir>/atlas_offload_<slot_id>.json`.
pub struct SoftwareNvmeAdapter {
    dir: PathBuf,
    next_slot: std::sync::atomic::AtomicU64,
}

impl SoftwareNvmeAdapter {
    /// Create an adapter that stores blobs in `dir`.
    pub fn new(dir: impl Into<PathBuf>) -> Self {
        let dir = dir.into();
        std::fs::create_dir_all(&dir).ok();
        Self {
            dir,
            next_slot: std::sync::atomic::AtomicU64::new(1),
        }
    }

    fn blob_path(&self, slot_id: u64) -> PathBuf {
        self.dir.join(format!("atlas_offload_{slot_id}.json"))
    }
}

impl NvmeOffloadAdapter for SoftwareNvmeAdapter {
    fn offload(&self, state: &AgentState) -> Result<OffloadHandle, OffloadError> {
        let slot_id = self
            .next_slot
            .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        let path = self.blob_path(slot_id);
        let bytes = serde_json::to_vec(state)
            .map_err(|e| OffloadError::Serialization(e.to_string()))?;
        std::fs::write(&path, bytes).map_err(|e| OffloadError::Io(e.to_string()))?;
        debug!("offloaded agent state for '{}' to {:?}", state.agent_id, path);
        Ok(OffloadHandle { slot_id })
    }

    fn restore(&self, handle: &OffloadHandle) -> Result<AgentState, OffloadError> {
        let path = self.blob_path(handle.slot_id);
        let bytes = std::fs::read(&path).map_err(|e| OffloadError::Io(e.to_string()))?;
        serde_json::from_slice(&bytes).map_err(|e| OffloadError::Serialization(e.to_string()))
    }

    fn drop_offload(&self, handle: &OffloadHandle) -> Result<(), OffloadError> {
        let path = self.blob_path(handle.slot_id);
        std::fs::remove_file(&path).map_err(|e| OffloadError::Io(e.to_string()))
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn offload_and_restore_round_trip() {
        let dir = std::env::temp_dir().join("atlas_offload_test");
        let adapter = SoftwareNvmeAdapter::new(&dir);

        let state = AgentState {
            agent_id: "reasoning-agent-0".into(),
            context_blob: b"cached_context_data".to_vec(),
            snapshotted_at_ns: 1_700_000_000_000,
        };

        let handle = adapter.offload(&state).expect("offload failed");
        let restored = adapter.restore(&handle).expect("restore failed");

        assert_eq!(restored.agent_id, state.agent_id);
        assert_eq!(restored.context_blob, state.context_blob);

        adapter.drop_offload(&handle).expect("drop failed");
        assert!(!adapter.blob_path(handle.slot_id).exists());
    }
}
