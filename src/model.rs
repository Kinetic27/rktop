use std::fmt;
use std::time::{Duration, SystemTime};

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostSource {
    Local,
    Ssh { host: String },
    Proxmox { host: String },
    TrueNasScale { host: String },
}

impl HostSource {
    pub fn local() -> Self {
        Self::Local
    }

    pub fn ssh(host: impl Into<String>) -> Self {
        Self::Ssh { host: host.into() }
    }

    pub fn proxmox(host: impl Into<String>) -> Self {
        Self::Proxmox { host: host.into() }
    }

    pub fn truenas_scale(host: impl Into<String>) -> Self {
        Self::TrueNasScale { host: host.into() }
    }

    pub fn kind(&self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::Ssh { .. } => "ssh",
            Self::Proxmox { .. } => "proxmox",
            Self::TrueNasScale { .. } => "truenas-scale",
        }
    }

    pub fn endpoint(&self) -> Option<&str> {
        match self {
            Self::Local => None,
            Self::Ssh { host } | Self::Proxmox { host } | Self::TrueNasScale { host } => {
                Some(host.as_str())
            }
        }
    }
}

impl fmt::Display for HostSource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Local => f.write_str("local"),
            Self::Ssh { host } => write!(f, "ssh:{host}"),
            Self::Proxmox { host } => write!(f, "proxmox:{host}"),
            Self::TrueNasScale { host } => write!(f, "truenas-scale:{host}"),
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CpuMetrics {
    pub usage_percent: Option<f32>,
    pub load_1m: Option<f32>,
    pub load_5m: Option<f32>,
    pub load_15m: Option<f32>,
    pub cores: Option<u16>,
    pub temperature_celsius: Option<f32>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RamMetrics {
    pub total_kib: Option<u64>,
    pub available_kib: Option<u64>,
    pub used_kib: Option<u64>,
}

impl RamMetrics {
    pub fn usage_percent(&self) -> Option<f32> {
        let total = self.total_kib?;
        if total == 0 {
            return None;
        }
        let used = self.used_kib.or_else(|| {
            self.available_kib
                .map(|available| total.saturating_sub(available))
        })?;
        Some((used as f32 / total as f32) * 100.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkMetrics {
    pub rx_bytes_total: Option<u64>,
    pub tx_bytes_total: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StorageMetrics {
    pub root_total_kib: Option<u64>,
    pub root_used_kib: Option<u64>,
    pub root_available_kib: Option<u64>,
    pub disks: Vec<DiskMetrics>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DiskMetrics {
    pub mount: String,
    pub total_kib: u64,
    pub used_kib: u64,
    pub available_kib: u64,
}

impl StorageMetrics {
    pub fn usage_percent(&self) -> Option<f32> {
        let total = self.root_total_kib?;
        if total == 0 {
            return None;
        }
        Some((self.root_used_kib? as f32 / total as f32) * 100.0)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Freshness {
    pub collected_at: SystemTime,
    pub stale_after: Duration,
}

impl Freshness {
    pub fn now(stale_after: Duration) -> Self {
        Self {
            collected_at: SystemTime::now(),
            stale_after,
        }
    }

    pub fn is_stale_at(&self, now: SystemTime) -> bool {
        now.duration_since(self.collected_at)
            .map(|age| age > self.stale_after)
            .unwrap_or(false)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum HostStatus {
    Online,
    Degraded,
    Offline,
    Disabled,
    Unknown,
}

#[derive(Clone, Debug, PartialEq)]
pub struct HostMetrics {
    pub id: String,
    pub name: String,
    pub hostname: Option<String>,
    pub kernel: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub group: Option<String>,
    pub role: Option<String>,
    pub source: HostSource,
    pub status: HostStatus,
    pub error: Option<String>,
    pub freshness: Freshness,
    pub cpu: CpuMetrics,
    pub ram: RamMetrics,
    pub network: NetworkMetrics,
    pub storage: StorageMetrics,
}

impl HostMetrics {
    pub fn disabled(
        id: impl Into<String>,
        name: impl Into<String>,
        source: HostSource,
        group: Option<String>,
        role: Option<String>,
    ) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            hostname: None,
            kernel: None,
            uptime_seconds: None,
            group,
            role,
            source,
            status: HostStatus::Disabled,
            error: Some("host disabled in config".to_string()),
            freshness: Freshness::now(Duration::from_secs(60)),
            cpu: CpuMetrics::empty(),
            ram: RamMetrics::empty(),
            network: NetworkMetrics::empty(),
            storage: StorageMetrics::empty(),
        }
    }
}

impl CpuMetrics {
    pub fn empty() -> Self {
        Self {
            usage_percent: None,
            load_1m: None,
            load_5m: None,
            load_15m: None,
            cores: None,
            temperature_celsius: None,
        }
    }
}

impl RamMetrics {
    pub fn empty() -> Self {
        Self {
            total_kib: None,
            available_kib: None,
            used_kib: None,
        }
    }
}

impl NetworkMetrics {
    pub fn empty() -> Self {
        Self {
            rx_bytes_total: None,
            tx_bytes_total: None,
        }
    }
}

impl StorageMetrics {
    pub fn empty() -> Self {
        Self {
            root_total_kib: None,
            root_used_kib: None,
            root_available_kib: None,
            disks: Vec::new(),
        }
    }
}
