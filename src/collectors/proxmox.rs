//! Proxmox adapter placeholder.
//!
//! The MVP starts with read-only local and SSH collection. Proxmox-specific API
//! collection can be added later without changing the shared HostMetrics model.

use crate::collectors::CollectorError;

pub fn collect() -> Result<(), CollectorError> {
    Err(CollectorError::UnsupportedSource(
        "proxmox API collector is not part of the SSH/local MVP".to_string(),
    ))
}
