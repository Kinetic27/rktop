//! TrueNAS adapter placeholder.
//!
//! TrueNAS SCALE 25.04+ uses JSON-RPC 2.0 over WebSocket for API integration.
//! The deprecated REST API is intentionally not used for the MVP. This lane only
//! enables read-only local and SSH collection and performs no remote writes,
//! installs, or token management.

use crate::collectors::CollectorError;

pub fn collect() -> Result<(), CollectorError> {
    Err(CollectorError::UnsupportedSource(
        "TrueNAS SCALE 25.04+ JSON-RPC 2.0 over WebSocket collector is scaffolded; deprecated REST is not used for MVP".to_string(),
    ))
}
