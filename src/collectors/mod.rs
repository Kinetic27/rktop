pub mod local;
pub mod proxmox;
pub mod ssh;
pub mod truenas;

use std::fmt;
use std::io;
use std::time::Duration;

use crate::config::ServerConfig;
use crate::model::{
    CpuMetrics, DiskMetrics, Freshness, HostMetrics, HostSource, HostStatus, NetworkMetrics,
    RamMetrics, StorageMetrics,
};

pub const DEFAULT_STALE_AFTER: Duration = Duration::from_secs(15);

#[derive(Debug)]
pub enum CollectorError {
    DisabledHost(String),
    UnsupportedSource(String),
    Io(io::Error),
    Utf8(std::string::FromUtf8Error),
    CommandFailed { code: Option<i32>, stderr: String },
}

impl fmt::Display for CollectorError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DisabledHost(id) => write!(f, "host '{id}' is disabled"),
            Self::UnsupportedSource(source) => write!(f, "unsupported source for MVP: {source}"),
            Self::Io(error) => write!(f, "collector I/O error: {error}"),
            Self::Utf8(error) => write!(f, "collector output was not UTF-8: {error}"),
            Self::CommandFailed { code, stderr } => {
                write!(f, "collector command failed (code {code:?}): {stderr}")
            }
        }
    }
}

impl std::error::Error for CollectorError {}

impl From<io::Error> for CollectorError {
    fn from(value: io::Error) -> Self {
        Self::Io(value)
    }
}

impl From<std::string::FromUtf8Error> for CollectorError {
    fn from(value: std::string::FromUtf8Error) -> Self {
        Self::Utf8(value)
    }
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RawHostSnapshot {
    pub hostname: Option<String>,
    pub kernel: Option<String>,
    pub uptime_seconds: Option<u64>,
    pub loadavg: Option<String>,
    pub cpu_cores: Option<u16>,
    pub cpu_temp_millicelsius: Option<u64>,
    pub mem_total_kib: Option<u64>,
    pub mem_available_kib: Option<u64>,
    pub net_rx_bytes: Option<u64>,
    pub net_tx_bytes: Option<u64>,
    pub root_total_kib: Option<u64>,
    pub root_used_kib: Option<u64>,
    pub root_available_kib: Option<u64>,
    pub disks: Vec<DiskMetrics>,
}

pub fn collect_host(server: &ServerConfig) -> Result<HostMetrics, CollectorError> {
    if !server.enabled {
        return Err(CollectorError::DisabledHost(server.id.clone()));
    }

    match &server.source {
        HostSource::Local => local::collect(server),
        HostSource::Ssh { host } => ssh::collect(server, host),
        source => Err(CollectorError::UnsupportedSource(source.to_string())),
    }
}

pub fn metrics_from_key_values(
    server: &ServerConfig,
    source: HostSource,
    output: &str,
) -> HostMetrics {
    let snapshot = parse_key_values(output);
    metrics_from_snapshot(server, source, snapshot)
}

pub fn metrics_from_snapshot(
    server: &ServerConfig,
    source: HostSource,
    snapshot: RawHostSnapshot,
) -> HostMetrics {
    let (load_1m, load_5m, load_15m) = snapshot
        .loadavg
        .as_deref()
        .map(parse_loadavg)
        .unwrap_or((None, None, None));

    let used_kib = match (snapshot.mem_total_kib, snapshot.mem_available_kib) {
        (Some(total), Some(available)) => Some(total.saturating_sub(available)),
        _ => None,
    };

    HostMetrics {
        id: server.id.clone(),
        name: server.name.clone(),
        hostname: snapshot.hostname,
        kernel: snapshot.kernel,
        uptime_seconds: snapshot.uptime_seconds,
        group: server.group.clone(),
        role: server.role.clone(),
        source,
        status: HostStatus::Online,
        error: None,
        freshness: Freshness::now(DEFAULT_STALE_AFTER),
        cpu: CpuMetrics {
            usage_percent: None,
            load_1m,
            load_5m,
            load_15m,
            cores: snapshot.cpu_cores,
            temperature_celsius: snapshot
                .cpu_temp_millicelsius
                .map(|temp| temp as f32 / 1000.0),
        },
        ram: RamMetrics {
            total_kib: snapshot.mem_total_kib,
            available_kib: snapshot.mem_available_kib,
            used_kib,
        },
        network: NetworkMetrics {
            rx_bytes_total: snapshot.net_rx_bytes,
            tx_bytes_total: snapshot.net_tx_bytes,
        },
        storage: StorageMetrics {
            root_total_kib: snapshot.root_total_kib,
            root_used_kib: snapshot.root_used_kib,
            root_available_kib: snapshot.root_available_kib,
            disks: snapshot.disks,
        },
    }
}

pub fn parse_key_values(output: &str) -> RawHostSnapshot {
    let mut snapshot = RawHostSnapshot::default();

    for line in output.lines() {
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = value.trim();
        match key.trim() {
            "hostname" => snapshot.hostname = non_empty(value),
            "kernel" => snapshot.kernel = non_empty(value),
            "uptime_seconds" => snapshot.uptime_seconds = parse_u64_value(value),
            "loadavg" => snapshot.loadavg = non_empty(value),
            "cpu_cores" => snapshot.cpu_cores = value.parse().ok(),
            "cpu_temp_millicelsius" => snapshot.cpu_temp_millicelsius = parse_u64_value(value),
            "mem_total_kib" => snapshot.mem_total_kib = parse_u64_value(value),
            "mem_available_kib" => snapshot.mem_available_kib = parse_u64_value(value),
            "net_rx_bytes" => snapshot.net_rx_bytes = parse_u64_value(value),
            "net_tx_bytes" => snapshot.net_tx_bytes = parse_u64_value(value),
            "root_total_kib" => snapshot.root_total_kib = parse_u64_value(value),
            "root_used_kib" => snapshot.root_used_kib = parse_u64_value(value),
            "root_available_kib" => snapshot.root_available_kib = parse_u64_value(value),
            "disk" => {
                if let Some(disk) = parse_disk_value(value) {
                    snapshot.disks.push(disk);
                }
            }
            _ => {}
        }
    }

    snapshot
}

fn parse_loadavg(loadavg: &str) -> (Option<f32>, Option<f32>, Option<f32>) {
    let mut fields = loadavg
        .split_whitespace()
        .filter_map(|field| field.parse().ok());
    (fields.next(), fields.next(), fields.next())
}

fn non_empty(value: &str) -> Option<String> {
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

fn parse_u64_value(value: &str) -> Option<u64> {
    value.parse().ok().or_else(|| {
        let parsed = value.parse::<f64>().ok()?;
        if parsed.is_finite() && parsed >= 0.0 && parsed <= u64::MAX as f64 {
            Some(parsed.round() as u64)
        } else {
            None
        }
    })
}

fn parse_disk_value(value: &str) -> Option<DiskMetrics> {
    let mut fields = value.split('|');
    let mount = fields.next()?.trim();
    let total_kib = parse_u64_value(fields.next()?.trim())?;
    let used_kib = parse_u64_value(fields.next()?.trim())?;
    let available_kib = parse_u64_value(fields.next()?.trim())?;
    if mount.is_empty() || total_kib == 0 {
        return None;
    }
    Some(DiskMetrics {
        mount: mount.to_string(),
        total_kib,
        used_kib,
        available_kib,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::ServerConfig;

    #[test]
    fn parses_collector_key_values() {
        let snapshot = parse_key_values(
            "hostname=box\nkernel=Linux x\nuptime_seconds=4406700\nloadavg=1.00 0.50 0.25 1/2 3\ncpu_cores=8\ncpu_temp_millicelsius=88600\nmem_total_kib=100\nmem_available_kib=40\nnet_rx_bytes=7\nnet_tx_bytes=9\nroot_total_kib=1000\nroot_used_kib=250\nroot_available_kib=750\n",
        );
        assert_eq!(snapshot.hostname.as_deref(), Some("box"));
        assert_eq!(snapshot.cpu_cores, Some(8));
        assert_eq!(snapshot.cpu_temp_millicelsius, Some(88_600));
        assert_eq!(snapshot.uptime_seconds, Some(4_406_700));
        assert_eq!(snapshot.mem_available_kib, Some(40));
        assert_eq!(snapshot.root_used_kib, Some(250));
    }

    #[test]
    fn parses_multiple_disk_lines() {
        let snapshot = parse_key_values("disk=/|1000|250|750\ndisk=/mnt/tank|2000|1200|800\n");
        assert_eq!(snapshot.disks.len(), 2);
        assert_eq!(snapshot.disks[0].mount, "/");
        assert_eq!(snapshot.disks[1].mount, "/mnt/tank");
        assert_eq!(snapshot.disks[1].used_kib, 1200);
    }

    #[test]
    fn parses_scientific_notation_network_counters() {
        let snapshot = parse_key_values("net_rx_bytes=3.2333e+12\nnet_tx_bytes=3.46015e+12\n");
        assert_eq!(snapshot.net_rx_bytes, Some(3_233_300_000_000));
        assert_eq!(snapshot.net_tx_bytes, Some(3_460_150_000_000));
    }

    #[test]
    fn builds_metrics_from_snapshot() {
        let server = ServerConfig::local("current", "current");
        let metrics = metrics_from_key_values(
            &server,
            HostSource::Local,
            "loadavg=1 2 3\ncpu_temp_millicelsius=88600\nmem_total_kib=100\nmem_available_kib=25\n",
        );
        assert_eq!(metrics.ram.used_kib, Some(75));
        assert_eq!(metrics.cpu.load_5m, Some(2.0));
        assert!(
            (metrics.cpu.temperature_celsius.unwrap() - 88.6).abs() < 0.01,
            "millidegree Celsius sensor value should be converted to °C"
        );
    }
}
