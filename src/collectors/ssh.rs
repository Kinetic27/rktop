use std::io::{self, Read};
use std::process::{Command, Output, Stdio};
use std::sync::mpsc;
use std::thread::{self, sleep};
use std::time::{Duration, Instant};

use crate::collectors::{CollectorError, metrics_from_key_values};
use crate::config::ServerConfig;
use crate::model::{HostMetrics, HostSource};

const SSH_TIMEOUT: Duration = Duration::from_secs(10);
const SSH_POLL_INTERVAL: Duration = Duration::from_millis(25);

const SSH_OPTIONS: &[(&str, &str)] = &[
    ("BatchMode", "yes"),
    ("PasswordAuthentication", "no"),
    ("KbdInteractiveAuthentication", "no"),
    ("ConnectTimeout", "5"),
    ("ConnectionAttempts", "1"),
    ("NumberOfPasswordPrompts", "0"),
];

const SSH_MULTIPLEX_OPTIONS: &[(&str, &str)] = &[
    ("ControlMaster", "auto"),
    ("ControlPersist", "10m"),
    ("ControlPath", "~/.ssh/rktop-%C"),
];

pub fn collect(server: &ServerConfig, host: &str) -> Result<HostMetrics, CollectorError> {
    validate_ssh_host(host)?;
    let output = run_ssh_script(host, crate::collectors::local::FIXED_COLLECT_COMMAND)?;
    if !output.success() {
        return Err(CollectorError::CommandFailed {
            code: output.code,
            stderr: String::from_utf8_lossy(&output.stderr).trim().to_string(),
        });
    }
    let stdout = String::from_utf8(output.stdout)?;
    Ok(metrics_from_key_values(
        server,
        HostSource::ssh(host.to_string()),
        &stdout,
    ))
}

pub fn ssh_command(host: &str) -> Command {
    let mut command = base_ssh_command(true);
    command.arg(host).arg(collect_payload_command());
    command
}

pub fn ssh_probe_command(host: &str) -> Command {
    let mut command = base_ssh_command(true);
    command.arg(host).arg("true");
    command
}

#[derive(Debug)]
struct SshCommandOutput {
    code: Option<i32>,
    stdout: Vec<u8>,
    stderr: Vec<u8>,
}

impl SshCommandOutput {
    fn success(&self) -> bool {
        self.code == Some(0)
    }
}

fn run_ssh_script(host: &str, _script: &str) -> Result<SshCommandOutput, CollectorError> {
    if cfg!(windows) {
        return run_windows_key_value_collector(host);
    }

    let mut command = ssh_command(host);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run_command(command, SSH_TIMEOUT)?;
    Ok(SshCommandOutput {
        code: output.status.code(),
        stdout: output.stdout,
        stderr: output.stderr,
    })
}

pub fn run_ssh_probe(host: &str) -> Result<Output, CollectorError> {
    let mut command = ssh_probe_command(host);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    run_command(command, SSH_TIMEOUT)
}

fn run_windows_key_value_collector(host: &str) -> Result<SshCommandOutput, CollectorError> {
    let mut key_values = Vec::new();

    push_key_value(
        &mut key_values,
        "hostname",
        first_line(&windows_remote_stdout(
            host,
            "hostname 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "kernel",
        first_line(&windows_remote_stdout(
            host,
            "uname -sr 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "loadavg",
        first_line(&windows_remote_stdout(
            host,
            "cat /proc/loadavg 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "uptime_seconds",
        first_line(&windows_remote_stdout(
            host,
            "awk '{printf \"%.0f\\n\", $1}' /proc/uptime 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "cpu_cores",
        first_line(&windows_remote_stdout(
            host,
            "grep -c '^processor' /proc/cpuinfo 2>/dev/null || printf '0'",
        )?),
    );
    push_key_value(
        &mut key_values,
        "cpu_temp_millicelsius",
        first_line(&windows_remote_stdout(host, CPU_TEMP_COMMAND).unwrap_or_default()),
    );
    push_key_value(
        &mut key_values,
        "mem_total_kib",
        first_line(&windows_remote_stdout(
            host,
            "awk '/^MemTotal:/ {print $2}' /proc/meminfo 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "mem_available_kib",
        first_line(&windows_remote_stdout(
            host,
            "awk '/^MemAvailable:/ {print $2}' /proc/meminfo 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "net_rx_bytes",
        first_line(&windows_remote_stdout(
            host,
            "awk 'NR>2 {gsub(\":\", \"\", $1); if ($1 != \"lo\") rx += $2} END {printf \"%.0f\\n\", rx + 0}' /proc/net/dev 2>/dev/null || true",
        )?),
    );
    push_key_value(
        &mut key_values,
        "net_tx_bytes",
        first_line(&windows_remote_stdout(
            host,
            "awk 'NR>2 {gsub(\":\", \"\", $1); if ($1 != \"lo\") tx += $10} END {printf \"%.0f\\n\", tx + 0}' /proc/net/dev 2>/dev/null || true",
        )?),
    );

    let root = windows_remote_stdout(
        host,
        "df -kP / 2>/dev/null | awk 'NR==2 {print $2 \" \" $3 \" \" $4}'",
    )?;
    let mut root_fields = first_line(&root).split_whitespace();
    push_key_value(
        &mut key_values,
        "root_total_kib",
        root_fields.next().unwrap_or("0"),
    );
    push_key_value(
        &mut key_values,
        "root_used_kib",
        root_fields.next().unwrap_or("0"),
    );
    push_key_value(
        &mut key_values,
        "root_available_kib",
        root_fields.next().unwrap_or("0"),
    );

    for line in windows_remote_stdout(host, ZPOOL_DISK_COMMAND)
        .unwrap_or_default()
        .lines()
    {
        push_key_value(&mut key_values, "disk", line.trim());
    }
    for line in windows_remote_stdout(host, DF_DISK_COMMAND)?.lines() {
        push_key_value(&mut key_values, "disk", line.trim());
    }

    Ok(SshCommandOutput {
        code: Some(0),
        stdout: key_values.join("\n").into_bytes(),
        stderr: Vec::new(),
    })
}

const WINDOWS_REMOTE_TIMEOUT: Duration = Duration::from_secs(2);
const WINDOWS_REMOTE_QUIET_AFTER_OUTPUT: Duration = Duration::from_millis(200);

const CPU_TEMP_COMMAND: &str = r#"for hwmon in /sys/class/hwmon/hwmon*; do
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
done | sort -n | tail -1"#;

const ZPOOL_DISK_COMMAND: &str = r#"if command -v zpool >/dev/null 2>&1; then
  zpool list -Hp -o name,size,alloc,free 2>/dev/null | while read -r pool size alloc free; do
    [ -n "$pool" ] || continue
    [ "$pool" = "boot-pool" ] && continue
    mount="/mnt/$pool"
    [ -d "$mount" ] || continue
    printf '%s|%s|%s|%s\n' "$mount" "$((size / 1024))" "$((alloc / 1024))" "$((free / 1024))"
  done
fi"#;

const DF_DISK_COMMAND: &str = "df -kP -x tmpfs -x devtmpfs -x squashfs -x overlay -x efivarfs 2>/dev/null | awk 'NR>1 && $2 > 0 {print $6 \"|\" $2 \"|\" $3 \"|\" $4}'";

fn windows_remote_stdout(host: &str, command_text: &str) -> Result<String, CollectorError> {
    let mut command = base_ssh_command(true);
    command.arg(host).arg(command_text);
    command.stdout(Stdio::piped()).stderr(Stdio::piped());
    let output = run_windows_command_text(command, WINDOWS_REMOTE_TIMEOUT)?;
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn run_windows_command_text(
    mut command: Command,
    timeout: Duration,
) -> Result<SshCommandOutput, CollectorError> {
    let mut child = command.spawn()?;
    let (stdout_reader, stdout_rx) = read_pipe_chunks_in_background(child.stdout.take());
    let stderr_reader = read_pipe_in_background(child.stderr.take());
    let mut stdout = Vec::new();
    let started = Instant::now();
    let mut last_stdout_at: Option<Instant> = None;

    loop {
        let before = stdout.len();
        drain_stdout_chunks(&stdout_rx, &mut stdout)?;
        if stdout.len() != before {
            last_stdout_at = Some(Instant::now());
        }

        if let Some(status) = child.try_wait()? {
            join_chunk_reader(stdout_reader)?;
            drain_stdout_chunks(&stdout_rx, &mut stdout)?;
            return Ok(SshCommandOutput {
                code: status.code(),
                stdout,
                stderr: join_pipe_reader(stderr_reader)?,
            });
        }

        if last_stdout_at.is_some_and(|last| last.elapsed() >= WINDOWS_REMOTE_QUIET_AFTER_OUTPUT) {
            let _ = child.kill();
            let _ = child.wait();
            join_chunk_reader(stdout_reader)?;
            drain_stdout_chunks(&stdout_rx, &mut stdout)?;
            return Ok(SshCommandOutput {
                code: Some(0),
                stdout,
                stderr: join_pipe_reader(stderr_reader)?,
            });
        }

        if started.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait()?;
            join_chunk_reader(stdout_reader)?;
            drain_stdout_chunks(&stdout_rx, &mut stdout)?;
            let stderr = join_pipe_reader(stderr_reader)?;
            if !stdout.is_empty() {
                return Ok(SshCommandOutput {
                    code: Some(0),
                    stdout,
                    stderr,
                });
            }
            let mut message = format!("SSH command timed out after {}s", timeout.as_secs());
            let detail = String::from_utf8_lossy(&stderr).trim().to_string();
            if !detail.is_empty() {
                message.push_str(": ");
                message.push_str(&detail);
            }
            return Err(CollectorError::CommandFailed {
                code: status.code(),
                stderr: message,
            });
        }
        sleep(SSH_POLL_INTERVAL);
    }
}

fn push_key_value(lines: &mut Vec<String>, key: &str, value: &str) {
    lines.push(format!("{key}={}", value.trim()));
}

fn first_line(output: &str) -> &str {
    output.lines().next().unwrap_or_default().trim()
}

fn drain_stdout_chunks(
    rx: &mpsc::Receiver<io::Result<Vec<u8>>>,
    stdout: &mut Vec<u8>,
) -> io::Result<()> {
    loop {
        match rx.try_recv() {
            Ok(Ok(chunk)) => stdout.extend_from_slice(&chunk),
            Ok(Err(err)) => return Err(err),
            Err(mpsc::TryRecvError::Empty) | Err(mpsc::TryRecvError::Disconnected) => return Ok(()),
        }
    }
}

fn run_command(mut command: Command, timeout: Duration) -> Result<Output, CollectorError> {
    let mut child = command.spawn()?;
    let stdout_reader = read_pipe_in_background(child.stdout.take());
    let stderr_reader = read_pipe_in_background(child.stderr.take());

    let started = Instant::now();
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if started.elapsed() >= timeout {
            let _ = child.kill();
            let status = child.wait()?;
            let _stdout = join_pipe_reader(stdout_reader)?;
            let stderr = join_pipe_reader(stderr_reader)?;
            let mut message = format!("SSH command timed out after {}s", timeout.as_secs());
            let detail = String::from_utf8_lossy(&stderr).trim().to_string();
            if !detail.is_empty() {
                message.push_str(": ");
                message.push_str(&detail);
            }
            return Err(CollectorError::CommandFailed {
                code: status.code(),
                stderr: message,
            });
        }
        sleep(SSH_POLL_INTERVAL);
    };

    Ok(Output {
        status,
        stdout: join_pipe_reader(stdout_reader)?,
        stderr: join_pipe_reader(stderr_reader)?,
    })
}

fn read_pipe_in_background<T>(pipe: Option<T>) -> thread::JoinHandle<io::Result<Vec<u8>>>
where
    T: Read + Send + 'static,
{
    thread::spawn(move || {
        let mut data = Vec::new();
        if let Some(mut pipe) = pipe {
            pipe.read_to_end(&mut data)?;
        }
        Ok(data)
    })
}

fn read_pipe_chunks_in_background<T>(
    pipe: Option<T>,
) -> (
    thread::JoinHandle<io::Result<()>>,
    mpsc::Receiver<io::Result<Vec<u8>>>,
)
where
    T: Read + Send + 'static,
{
    let (tx, rx) = mpsc::channel();
    let reader = thread::spawn(move || {
        if let Some(mut pipe) = pipe {
            let mut buffer = [0_u8; 8192];
            loop {
                match pipe.read(&mut buffer) {
                    Ok(0) => break,
                    Ok(read) => {
                        if tx.send(Ok(buffer[..read].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(err) => {
                        let _ = tx.send(Err(err));
                        break;
                    }
                }
            }
        }
        Ok(())
    });
    (reader, rx)
}

fn join_chunk_reader(reader: thread::JoinHandle<io::Result<()>>) -> io::Result<()> {
    reader
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("SSH pipe chunk reader thread panicked")))
}

fn join_pipe_reader(reader: thread::JoinHandle<io::Result<Vec<u8>>>) -> io::Result<Vec<u8>> {
    reader
        .join()
        .unwrap_or_else(|_| Err(io::Error::other("SSH pipe reader thread panicked")))
}

fn collect_payload_command() -> String {
    if cfg!(windows) {
        let encoded = base64_encode(crate::collectors::local::FIXED_COLLECT_COMMAND.as_bytes());
        format!("printf '%s' {encoded} | base64 -d | sh")
    } else {
        crate::collectors::local::FIXED_COLLECT_COMMAND.to_string()
    }
}

fn base64_encode(input: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut encoded = String::with_capacity(input.len().div_ceil(3) * 4);
    for chunk in input.chunks(3) {
        let b0 = chunk[0];
        let b1 = *chunk.get(1).unwrap_or(&0);
        let b2 = *chunk.get(2).unwrap_or(&0);
        encoded.push(TABLE[(b0 >> 2) as usize] as char);
        encoded.push(TABLE[(((b0 & 0b0000_0011) << 4) | (b1 >> 4)) as usize] as char);
        if chunk.len() > 1 {
            encoded.push(TABLE[(((b1 & 0b0000_1111) << 2) | (b2 >> 6)) as usize] as char);
        } else {
            encoded.push('=');
        }
        if chunk.len() > 2 {
            encoded.push(TABLE[(b2 & 0b0011_1111) as usize] as char);
        } else {
            encoded.push('=');
        }
    }
    encoded
}

fn base_ssh_command(detach_stdin: bool) -> Command {
    let mut command = Command::new("ssh");
    if detach_stdin {
        command.arg("-n");
    }
    for (key, value) in SSH_OPTIONS {
        command.arg("-o").arg(format!("{key}={value}"));
    }
    if !cfg!(windows) {
        for (key, value) in SSH_MULTIPLEX_OPTIONS {
            command.arg("-o").arg(format!("{key}={value}"));
        }
    }
    command
}

pub fn validate_ssh_host(host: &str) -> Result<(), CollectorError> {
    let valid = !host.is_empty()
        && !host.starts_with('-')
        && host
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '.' | '-' | '_' | '@'));

    if valid {
        Ok(())
    } else {
        Err(CollectorError::UnsupportedSource(format!(
            "invalid SSH host alias: {host:?}"
        )))
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn ssh_collector_uses_non_interactive_stdin_script_command() {
        super::validate_ssh_host("ExampleHost").unwrap();
        let debug = format!("{:?}", super::ssh_command("ExampleHost"));
        assert!(debug.contains("BatchMode=yes"));
        assert!(debug.contains("PasswordAuthentication=no"));
        assert!(debug.contains("KbdInteractiveAuthentication=no"));
        assert!(debug.contains("ConnectTimeout=5"));
        assert!(debug.contains("ConnectionAttempts=1"));
        assert!(debug.contains("NumberOfPasswordPrompts=0"));
        assert!(debug.contains("ExampleHost"));
        if cfg!(windows) {
            assert!(debug.contains("base64 -d | sh"));
            assert!(!debug.contains("/proc/loadavg"));
        } else {
            assert!(debug.contains("/proc/loadavg"));
        }
    }

    #[test]
    fn ssh_probe_uses_shared_non_interactive_options_without_collection_script() {
        super::validate_ssh_host("ExampleHost").unwrap();
        let debug = format!("{:?}", super::ssh_probe_command("ExampleHost"));
        assert!(debug.contains("BatchMode=yes"));
        assert!(debug.contains("PasswordAuthentication=no"));
        assert!(debug.contains("KbdInteractiveAuthentication=no"));
        assert!(debug.contains("ConnectTimeout=5"));
        assert!(debug.contains("ConnectionAttempts=1"));
        assert!(debug.contains("NumberOfPasswordPrompts=0"));
        assert!(debug.contains("ExampleHost"));
        assert!(debug.contains("true"));
        assert!(!debug.contains("/proc/loadavg"));
    }

    #[test]
    fn ssh_multiplexing_is_disabled_on_windows() {
        let debug = format!("{:?}", super::ssh_command("ExampleHost"));
        if cfg!(windows) {
            assert!(!debug.contains("ControlMaster=auto"));
            assert!(!debug.contains("ControlPersist=10m"));
            assert!(!debug.contains("ControlPath=~/.ssh/rktop-%C"));
        } else {
            assert!(debug.contains("ControlMaster=auto"));
            assert!(debug.contains("ControlPersist=10m"));
            assert!(debug.contains("ControlPath=~/.ssh/rktop-%C"));
        }
    }

    #[test]
    fn base64_encoder_matches_expected_padding() {
        assert_eq!(super::base64_encode(b""), "");
        assert_eq!(super::base64_encode(b"f"), "Zg==");
        assert_eq!(super::base64_encode(b"fo"), "Zm8=");
        assert_eq!(super::base64_encode(b"foo"), "Zm9v");
        assert_eq!(super::base64_encode(b"hello\n"), "aGVsbG8K");
    }

    #[test]
    fn rejects_option_like_hosts() {
        assert!(super::validate_ssh_host("-oProxyCommand=x").is_err());
        assert!(super::validate_ssh_host("host name").is_err());
    }
}
