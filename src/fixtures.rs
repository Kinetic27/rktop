use std::time::{Duration, UNIX_EPOCH};

use crate::config::{AppConfig, ServerConfig, default_servers};
use crate::model::{
    CpuMetrics, DiskMetrics, Freshness, HostMetrics, HostSource, HostStatus, NetworkMetrics,
    RamMetrics, StorageMetrics,
};

const FIXTURE_TIME_SECS: u64 = 1_798_838_400; // 2027-01-01T00:00:00Z, stable display ordering only.

pub fn fixture_hosts() -> Vec<HostMetrics> {
    fixture_hosts_for_servers(default_servers())
}

pub fn fixture_hosts_for_config(config: &AppConfig) -> Vec<HostMetrics> {
    fixture_hosts_for_servers(config.servers.clone())
}

fn fixture_hosts_for_servers(servers: Vec<ServerConfig>) -> Vec<HostMetrics> {
    servers
        .into_iter()
        .filter(|server| server.enabled)
        .enumerate()
        .map(|(index, server)| fixture_for_server(&server, index as u64))
        .collect()
}

pub fn fixture_hosts_with_disabled() -> Vec<HostMetrics> {
    let mut hosts = fixture_hosts();
    hosts.push(disabled_optional_fixture());
    hosts
}

pub fn disabled_optional_fixture() -> HostMetrics {
    let server = ServerConfig::ssh("optional-server", "Optional Server", "optional-server")
        .with_group("Optional")
        .with_role("Optional Server")
        .optional(true)
        .enabled(false);
    disabled_fixture_for_server(&server)
}

pub fn fixture_for_server(server: &ServerConfig, offset: u64) -> HostMetrics {
    let base = offset + 1;
    HostMetrics {
        id: server.id.clone(),
        name: server.name.clone(),
        hostname: Some(match &server.source {
            HostSource::Local => "local-host".to_string(),
            HostSource::Ssh { host }
            | HostSource::Proxmox { host }
            | HostSource::TrueNasScale { host } => host.to_ascii_lowercase(),
        }),
        kernel: Some(format!("Linux 6.8.{}-fixture", base)),
        uptime_seconds: Some(base * 86_400 + 25 * 60),
        group: server.group.clone(),
        role: server.role.clone(),
        source: server.source.clone(),
        status: HostStatus::Online,
        error: None,
        freshness: fixture_freshness(),
        cpu: CpuMetrics {
            usage_percent: Some(10.0 + (base as f32 * 7.0)),
            load_1m: Some(0.10 * base as f32),
            load_5m: Some(0.08 * base as f32),
            load_15m: Some(0.05 * base as f32),
            cores: Some(2 + base as u16),
            temperature_celsius: fixture_cpu_temperature(&server.id, base),
        },
        ram: RamMetrics {
            total_kib: Some((8 + base) * 1024 * 1024),
            available_kib: Some((3 + base) * 1024 * 1024),
            used_kib: Some((5 + base) * 1024 * 1024),
        },
        network: NetworkMetrics {
            rx_bytes_total: Some(base * 1_000_000_000),
            tx_bytes_total: Some(base * 500_000_000),
        },
        storage: StorageMetrics {
            root_total_kib: Some(256 * 1024 * 1024),
            root_used_kib: Some((40 + base * 11) * 1024 * 1024),
            root_available_kib: Some((216 - base * 11) * 1024 * 1024),
            disks: fixture_disks(&server.id, base),
        },
    }
}

fn fixture_cpu_temperature(server_id: &str, base: u64) -> Option<f32> {
    match server_id {
        "server-1" => Some(74.0 + base as f32),
        "storage" => Some(58.0 + base as f32),
        _ => None,
    }
}

fn fixture_disks(server_id: &str, base: u64) -> Vec<DiskMetrics> {
    let root_used = (40 + base * 11) * 1024 * 1024;
    let mut disks = vec![DiskMetrics {
        mount: "/".to_string(),
        total_kib: 256 * 1024 * 1024,
        used_kib: root_used,
        available_kib: (256 * 1024 * 1024) - root_used,
    }];
    if server_id == "storage" {
        disks.extend([
            DiskMetrics {
                mount: "/mnt/tank".to_string(),
                total_kib: 8 * 1024 * 1024 * 1024,
                used_kib: 5 * 1024 * 1024 * 1024,
                available_kib: 3 * 1024 * 1024 * 1024,
            },
            DiskMetrics {
                mount: "/mnt/backup".to_string(),
                total_kib: 4 * 1024 * 1024 * 1024,
                used_kib: 1024 * 1024 * 1024,
                available_kib: 3 * 1024 * 1024 * 1024,
            },
        ]);
    }
    disks
}

fn disabled_fixture_for_server(server: &ServerConfig) -> HostMetrics {
    let mut host = HostMetrics::disabled(
        server.id.clone(),
        server.name.clone(),
        server.source.clone(),
        server.group.clone(),
        server.role.clone(),
    );
    host.hostname = Some("optional-server".to_string());
    host.error = Some("disabled fixture: optional server is offline".to_string());
    host.freshness = fixture_freshness();
    host
}

fn fixture_freshness() -> Freshness {
    Freshness {
        collected_at: UNIX_EPOCH + Duration::from_secs(FIXTURE_TIME_SECS),
        stale_after: Duration::from_secs(60),
    }
}
