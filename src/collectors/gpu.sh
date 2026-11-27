# Read-only GPU collection, shared by local and SSH hosts.
gpu_collect_nvidia() {
  command -v nvidia-smi >/dev/null 2>&1 || return 0
  command -v timeout >/dev/null 2>&1 || return 0
  if gpu_csv=$(LC_ALL=C timeout -k 1s 2s nvidia-smi --query-gpu=uuid,utilization.gpu,memory.used,memory.total,temperature.gpu,name --format=csv,noheader,nounits 2>/dev/null); then
    printf '%s\n' "$gpu_csv" | while IFS= read -r gpu_row; do
      [ -n "$gpu_row" ] && printf 'gpu_nvidia=%s\n' "$gpu_row"
    done
  fi
  return 0
}

gpu_collect_card() (
  card=$1
  vendor=$(cat "$card/device/vendor" 2>/dev/null)
  case "$vendor" in
    0x1002) brand=AMD ;;
    0x8086) brand=Intel ;;
    *) exit 0 ;;
  esac
  slot=$(awk -F= '$1 == "PCI_SLOT_NAME" {print $2}' "$card/device/uevent" 2>/dev/null)
  [ -n "$slot" ] || exit 0
  id="pci:$slot"
  device=$(cat "$card/device/device" 2>/dev/null)
  name=$(cat "$card/device/product_name" 2>/dev/null)
  if [ -z "$name" ] && command -v lspci >/dev/null 2>&1; then
    name=$(LC_ALL=C lspci -s "$slot" 2>/dev/null | sed 's/^[^ ]* [^:]*: //')
  fi
  [ -n "$name" ] || name="$brand $device"
  busy=$(cat "$card/device/gpu_busy_percent" 2>/dev/null)
  used=$(cat "$card/device/mem_info_vram_used" 2>/dev/null)
  total=$(cat "$card/device/mem_info_vram_total" 2>/dev/null)
  temp=$(for hwmon in "$card"/device/hwmon/hwmon*; do
    # amdgpu/i915 expose temp1; xe exposes GPU package temperature as temp2.
    for sensor in "$hwmon/temp1_input" "$hwmon/temp2_input"; do
      if [ -r "$sensor" ]; then
        cat "$sensor" 2>/dev/null
        break
      fi
    done
  done | sort -n | tail -1)
  printf 'gpu_drm=%s|%s|%s|%s|%s|%s\n' "$id" "$name" "$busy" "$used" "$total" "$temp"
  if [ "$brand" = Intel ] && command -v intel_gpu_top >/dev/null 2>&1 && command -v timeout >/dev/null 2>&1; then
    # SIGINT lets intel_gpu_top flush the last sample. Complete earlier samples
    # remain usable if it is killed midway through a subsequent JSON object.
    json=$(LC_ALL=C timeout -s INT -k 1s 0.4s intel_gpu_top -d "drm:/dev/dri/${card##*/}" -J -s 100 -o - 2>/dev/null)
    result=$?
    case "$result" in
      0|124) printf 'gpu_intel=%s|%s\n' "$id" "$(printf '%s' "$json" | tr -d '\n\r')" ;;
    esac
  fi
  exit 0
)

gpu_collect() {
  gpu_collect_nvidia
  for card in /sys/class/drm/card[0-9]*; do
    number=${card##*/card}
    case "$number" in ''|*[!0-9]*) continue ;; esac
    gpu_collect_card "$card"
  done
  return 0
}
