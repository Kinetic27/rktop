use std::process::Command;

use crate::collectors::{CollectorError, metrics_from_key_values};
use crate::config::ServerConfig;
use crate::model::{HostMetrics, HostSource};

pub fn collect(server: &ServerConfig) -> Result<HostMetrics, CollectorError> {
    let output = Command::new("sh")
        .arg("-c")
        .arg(FIXED_COLLECT_COMMAND)
        .output()?;
    if !output.status.success() {
        return Err(CollectorError::CommandFailed {
            code: output.status.code(),
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(metrics_from_key_values(server, HostSource::Local, &stdout))
}

pub const FIXED_COLLECT_COMMAND: &str = r#"hostname=$(hostname 2>/dev/null || true)
kernel=$(uname -sr 2>/dev/null || true)
loadavg=$(cat /proc/loadavg 2>/dev/null || true)
uptime_seconds=$(awk '{printf "%.0f\n", $1}' /proc/uptime 2>/dev/null || true)
cpu_cores=$(grep -c '^processor' /proc/cpuinfo 2>/dev/null || printf '0')
cpu_temp_millicelsius=$(for hwmon in /sys/class/hwmon/hwmon*; do
  [ -r "$hwmon/name" ] || continue
  name=$(cat "$hwmon/name" 2>/dev/null)
  case "$name" in
    coretemp|k10temp)
      for temp_file in "$hwmon"/temp*_input; do
        [ -r "$temp_file" ] || continue
        temp=$(cat "$temp_file" 2>/dev/null)
        case "$temp" in ''|*[!0-9]*) ;; *) printf '%s\n' "$temp" ;; esac
      done
      ;;
  esac
done | sort -n | tail -1)
mem_total_kib=$(awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null)
mem_available_kib=$(awk '/^MemAvailable:/ {print $2}' /proc/meminfo 2>/dev/null)
net_rx_bytes=$(awk 'NR>2 {gsub(":", "", $1); if ($1 != "lo") rx += $2} END {printf "%.0f\n", rx + 0}' /proc/net/dev 2>/dev/null)
net_tx_bytes=$(awk 'NR>2 {gsub(":", "", $1); if ($1 != "lo") tx += $10} END {printf "%.0f\n", tx + 0}' /proc/net/dev 2>/dev/null)
df_line=$(df -kP / 2>/dev/null | awk 'NR==2 {print $2 " " $3 " " $4}')
set -- $df_line
zpool_lines=$(if command -v zpool >/dev/null 2>&1; then
  zpool list -Hp -o name,size,alloc,free 2>/dev/null | while read -r pool size alloc free; do
    [ -n "$pool" ] || continue
    [ "$pool" = "boot-pool" ] && continue
    mount="/mnt/$pool"
    [ -d "$mount" ] || continue
    printf '%s|%s|%s|%s\n' "$mount" "$((size / 1024))" "$((alloc / 1024))" "$((free / 1024))"
  done
fi)
disk_lines=$(df -kP -x tmpfs -x devtmpfs -x squashfs -x overlay -x efivarfs 2>/dev/null | awk 'NR>1 && $2 > 0 {print $6 "|" $2 "|" $3 "|" $4}')
printf 'hostname=%s\n' "$hostname"
printf 'kernel=%s\n' "$kernel"
printf 'loadavg=%s\n' "$loadavg"
printf 'uptime_seconds=%s\n' "$uptime_seconds"
printf 'cpu_cores=%s\n' "$cpu_cores"
printf 'cpu_temp_millicelsius=%s\n' "$cpu_temp_millicelsius"
printf 'mem_total_kib=%s\n' "$mem_total_kib"
printf 'mem_available_kib=%s\n' "$mem_available_kib"
printf 'net_rx_bytes=%s\n' "$net_rx_bytes"
printf 'net_tx_bytes=%s\n' "$net_tx_bytes"
printf 'root_total_kib=%s\n' "${1:-0}"
printf 'root_used_kib=%s\n' "${2:-0}"
printf 'root_available_kib=%s\n' "${3:-0}"
printf '%s\n%s\n' "$zpool_lines" "$disk_lines" | while IFS= read -r disk_line; do
  [ -n "$disk_line" ] && printf 'disk=%s\n' "$disk_line"
done
"#;
