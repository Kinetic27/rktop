use crate::model::GpuMetrics;

pub(super) fn parse_nvidia(value: &str) -> Option<GpuMetrics> {
    let fields = value.splitn(6, ',').map(str::trim).collect::<Vec<_>>();
    if fields.len() != 6
        || !fields[0].starts_with("GPU-")
        || fields[0].len() <= 4
        || fields[5].is_empty()
    {
        return None;
    }
    Some(GpuMetrics {
        id: fields[0].to_string(),
        name: fields[5].to_string(),
        usage_percent: percent(fields[1]),
        memory_used_bytes: mib_to_bytes(fields[2]),
        memory_total_bytes: mib_to_bytes(fields[3]).filter(|total| *total > 0),
        temperature_celsius: temperature(fields[4], 1.0),
    })
}

pub(super) fn parse_drm(value: &str) -> Option<GpuMetrics> {
    let fields = value.split('|').map(str::trim).collect::<Vec<_>>();
    if fields.len() != 6
        || !fields[0].starts_with("pci:")
        || fields[0].len() <= 4
        || fields[1].is_empty()
    {
        return None;
    }
    Some(GpuMetrics {
        id: fields[0].to_string(),
        name: fields[1].to_string(),
        usage_percent: percent(fields[2]),
        memory_used_bytes: fields[3].parse().ok(),
        memory_total_bytes: fields[4].parse::<u64>().ok().filter(|total| *total > 0),
        temperature_celsius: temperature(fields[5], 1000.0),
    })
}

fn percent(value: &str) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .filter(|n| n.is_finite() && (0.0..=100.0).contains(n))
}

fn temperature(value: &str, divisor: f32) -> Option<f32> {
    value
        .parse::<f32>()
        .ok()
        .map(|n| n / divisor)
        .filter(|n| n.is_finite() && *n >= -273.15)
}

fn mib_to_bytes(value: &str) -> Option<u64> {
    let bytes = value.parse::<f64>().ok()? * 1_048_576.0;
    (bytes.is_finite() && bytes >= 0.0 && bytes < u64::MAX as f64).then_some(bytes.round() as u64)
}

// intel_gpu_top emits comma-separated objects, with or without an outer array
// depending on version. Only complete samples count; a truncated final object
// must not replace the last valid measurement. Use the busiest engine class,
// since adding concurrent engine percentages would double-count GPU time.
pub(super) fn intel_usage(mut json: &str) -> Option<f32> {
    let mut latest = None;
    loop {
        json = json.trim_start_matches(|c: char| c.is_whitespace() || c == ',' || c == '[');
        if json.is_empty() || json.starts_with(']') {
            break;
        }
        let mut stream = serde_json::Deserializer::from_str(json).into_iter::<serde_json::Value>();
        let Some(Ok(sample)) = stream.next() else {
            break;
        };
        latest = sample
            .get("engines")
            .and_then(|engines| engines.as_object())
            .and_then(|engines| {
                engines
                    .values()
                    .filter_map(|engine| {
                        let busy = engine.get("busy")?.as_f64()?;
                        (busy.is_finite() && (0.0..=100.0).contains(&busy)).then_some(busy as f32)
                    })
                    .max_by(f32::total_cmp)
            });
        json = &json[stream.byte_offset()..];
    }
    latest
}

#[cfg(test)]
mod tests {
    use crate::{collectors::metrics_from_key_values, config::ServerConfig, model::HostSource};

    #[test]
    fn unavailable_and_invalid_gpu_fields_do_not_become_zero() {
        let metrics = metrics_from_key_values(
            &ServerConfig::local("local", "Local"),
            HostSource::Local,
            "gpu_nvidia=GPU-a, N/A, [Not Supported], 8192, N/A, NVIDIA A\n\
             gpu_nvidia=GPU-b, 101, -1, NaN, inf, NVIDIA B\n\
             gpu_nvidia=diagnostic, 20, 1, 2, 40, bogus\n\
             gpu_nvidia=GPU-truncated, 20, 1\n\
             gpu_drm=pci:0000:03:00.0|AMD|NaN||0|\nmem_total_kib=100\nmem_available_kib=25\n",
        );
        assert_eq!(metrics.gpus.len(), 3);
        assert_eq!(metrics.gpus[0].memory_total_bytes, Some(8_589_934_592));
        for gpu in &metrics.gpus {
            assert_eq!(gpu.usage_percent, None);
            assert_eq!(gpu.memory_used_bytes, None);
            assert_eq!(gpu.temperature_celsius, None);
        }
        assert_eq!(metrics.gpus[1].memory_total_bytes, None);
        assert_eq!(metrics.gpus[2].memory_total_bytes, None);
        assert_eq!(metrics.ram.used_kib, Some(75));
    }

    #[test]
    fn intel_uses_last_complete_sample_and_busiest_engine_not_sum() {
        let samples = r#"{"engines":{"Render/3D/0":{"busy":12.0},"Video/0":{"busy":20.0}}},
            {"engines":{"Render/3D/0":{"busy":55.5},"Video/0":{"busy":24.0}}},
            {"engines":{"Render/3D/0":{"busy":99"#;
        assert_eq!(super::intel_usage(samples), Some(55.5));
        assert_eq!(super::intel_usage(&format!("[{samples}")), Some(55.5));
        assert_eq!(
            super::intel_usage(r#"[{"engines":{"Render/3D/0":{"busy":0.0}}}]"#),
            Some(0.0)
        );
        assert_eq!(
            super::intel_usage(r#"{"engines":{"Render/3D/0":{"busy":-1.0}}}"#),
            None
        );
        assert_eq!(super::intel_usage("Permission denied"), None);
    }

    #[cfg(target_os = "linux")]
    mod shell {
        use super::*;
        use std::{
            fs,
            os::unix::fs::PermissionsExt,
            path::PathBuf,
            process::Command,
            sync::atomic::{AtomicUsize, Ordering},
        };

        struct HostDir(PathBuf);

        impl HostDir {
            fn new() -> Self {
                static NEXT: AtomicUsize = AtomicUsize::new(0);
                let dir = std::env::temp_dir().join(format!(
                    "rktop-gpu-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
                fs::create_dir_all(dir.join("bin")).unwrap();
                Self(dir)
            }

            fn tool(&self, name: &str, script: &str) {
                let path = self.0.join("bin").join(name);
                fs::write(&path, format!("#!/bin/sh\n{script}\n")).unwrap();
                fs::set_permissions(path, fs::Permissions::from_mode(0o755)).unwrap();
            }

            fn card(&self, number: u8, vendor: &str, slot: &str) -> PathBuf {
                let card = self.0.join(format!("card{number}"));
                fs::create_dir_all(card.join("device/hwmon/hwmon0")).unwrap();
                fs::write(card.join("device/vendor"), vendor).unwrap();
                fs::write(
                    card.join("device/uevent"),
                    format!("PCI_SLOT_NAME={slot}\n"),
                )
                .unwrap();
                fs::write(
                    card.join("device/product_name"),
                    format!("Test GPU {number}"),
                )
                .unwrap();
                card
            }

            fn run(&self, calls: &str, system_tools: bool) -> String {
                let mut path = self.0.join("bin").into_os_string();
                if system_tools {
                    path.push(":/usr/bin:/bin");
                }
                let output = Command::new("/bin/sh")
                    .arg("-c")
                    .arg(format!("{}\n{calls}", include_str!("gpu.sh")))
                    .env("PATH", path)
                    .env("GPU_TEST_ROOT", &self.0)
                    .output()
                    .unwrap();
                assert!(
                    output.status.success(),
                    "{}",
                    String::from_utf8_lossy(&output.stderr)
                );
                String::from_utf8(output.stdout).unwrap()
            }
        }

        impl Drop for HostDir {
            fn drop(&mut self) {
                fs::remove_dir_all(&self.0).unwrap();
            }
        }

        #[test]
        fn collects_nvidia_amd_and_intel_through_real_shell_and_parser() {
            let host = HostDir::new();
            host.tool("nvidia-smi", "[ \"$1\" = '--query-gpu=uuid,utilization.gpu,memory.used,memory.total,temperature.gpu,name' ] || exit 1\n[ \"$2\" = '--format=csv,noheader,nounits' ] || exit 1\nprintf '%s\\n' 'GPU-a, 100, 512.5, 8192, 62, NVIDIA A, Special Edition' 'GPU-b, 0, 0, 4096, 40, NVIDIA B'");
            host.tool("intel_gpu_top", "[ \"$1\" = -d ] && [ \"$2\" = drm:/dev/dri/card2 ] && [ \"$3\" = -J ] || exit 1\ntrap '' INT\nprintf '%s\\n' '{\"engines\":{\"Render/3D/0\":{\"busy\":72.25},\"Video/0\":{\"busy\":19.0}}}'\nexec /bin/sleep 30");
            let amd = host.card(1, "0x1002", "0000:03:00.0");
            fs::write(amd.join("device/gpu_busy_percent"), "83\n").unwrap();
            fs::write(amd.join("device/mem_info_vram_used"), "1073741824\n").unwrap();
            fs::write(amd.join("device/mem_info_vram_total"), "8589934592\n").unwrap();
            fs::write(amd.join("device/hwmon/hwmon0/temp1_input"), "57500\n").unwrap();
            let intel = host.card(2, "0x8086", "0000:00:02.0");
            fs::write(intel.join("device/hwmon/hwmon0/temp2_input"), "43000\n").unwrap();
            let output = host.run("gpu_collect_nvidia\ngpu_collect_card \"$GPU_TEST_ROOT/card1\"\ngpu_collect_card \"$GPU_TEST_ROOT/card2\"", true);
            let metrics = metrics_from_key_values(
                &ServerConfig::local("local", "Local"),
                HostSource::Local,
                &output,
            );
            assert_eq!(metrics.gpus.len(), 4, "{output}");
            assert_eq!(metrics.gpus[0].name, "NVIDIA A, Special Edition");
            assert_eq!(metrics.gpus[0].usage_percent, Some(100.0));
            assert_eq!(metrics.gpus[0].memory_used_bytes, Some(537_395_200));
            assert_eq!(metrics.gpus[1].usage_percent, Some(0.0));
            assert_eq!(metrics.gpus[1].memory_used_bytes, Some(0));
            assert_eq!(metrics.gpus[2].id, "pci:0000:03:00.0");
            assert_eq!(metrics.gpus[2].usage_percent, Some(83.0));
            assert_eq!(metrics.gpus[2].memory_used_bytes, Some(1_073_741_824));
            assert_eq!(metrics.gpus[2].memory_total_bytes, Some(8_589_934_592));
            assert_eq!(metrics.gpus[2].temperature_celsius, Some(57.5));
            assert_eq!(metrics.gpus[3].id, "pci:0000:00:02.0");
            assert_eq!(metrics.gpus[3].usage_percent, Some(72.25));
            assert_eq!(metrics.gpus[3].memory_total_bytes, None);
            assert_eq!(metrics.gpus[3].temperature_celsius, Some(43.0));
        }

        #[test]
        fn failed_or_missing_nvidia_command_does_not_emit_partial_measurements() {
            let host = HostDir::new();
            host.tool(
                "nvidia-smi",
                "printf '%s\\n' 'GPU-a, 20, 100, 8192, 40, NVIDIA A'\nexit 1",
            );
            let calls = "gpu_collect_nvidia\nprintf 'hostname=still-online\\n'";
            for system_tools in [true, false] {
                let output = host.run(calls, system_tools);
                assert_eq!(output, "hostname=still-online\n");
            }
        }

        #[test]
        fn hung_nvidia_query_is_killed_and_does_not_block_other_metrics() {
            let host = HostDir::new();
            host.tool("nvidia-smi", "trap '' TERM\nprintf '%s\\n' 'GPU-a, 20, 100, 8192, 40, NVIDIA A'\nexec /bin/sleep 30");
            let started = std::time::Instant::now();
            let output = host.run(
                "gpu_collect_nvidia\nprintf 'hostname=still-online\\n'",
                true,
            );
            assert_eq!(output, "hostname=still-online\n");
            assert!(started.elapsed() < std::time::Duration::from_secs(10));
        }

        #[test]
        fn intel_permission_failure_preserves_device_without_fabricated_usage() {
            let host = HostDir::new();
            host.card(2, "0x8086", "0000:00:02.0");
            host.tool("intel_gpu_top", "printf 'Permission denied\\n' >&2\nexit 1");
            let output = host.run("gpu_collect_card \"$GPU_TEST_ROOT/card2\"", true);
            let metrics = metrics_from_key_values(
                &ServerConfig::local("local", "Local"),
                HostSource::Local,
                &output,
            );
            assert_eq!(metrics.gpus.len(), 1);
            assert_eq!(metrics.gpus[0].id, "pci:0000:00:02.0");
            assert_eq!(metrics.gpus[0].usage_percent, None);
        }
    }
}
