#!/bin/bash

rsscript_ramdisk_gib="${RSSCRIPT_RAMDISK_GIB:-8}"

rsscript_ramdisk_blocks() {
  awk -v gib="$rsscript_ramdisk_gib" 'BEGIN {printf "%d\n", gib * 1024 * 1024 * 1024 / 512}'
}

rsscript_is_mountpoint() {
  local path="$1"
  mount | awk -v path="$path" '$0 ~ " on " path " " {found = 1} END {exit found ? 0 : 1}'
}

rsscript_ensure_ramdisk_root() {
  if [[ -n "${RSSCRIPT_RAMDISK_PATH:-}" ]]; then
    echo "$RSSCRIPT_RAMDISK_PATH"
    return
  fi

  case "$(uname -s)" in
    Darwin)
      local path="/Volumes/RSScriptRAMDisk"
      if ! rsscript_is_mountpoint "$path"; then
        local device
        device="$(hdiutil attach -nomount "ram://$(rsscript_ramdisk_blocks)" | awk 'NF {print $1; exit}')"
        if [[ -z "$device" ]]; then
          echo "failed to create RSScript RAM disk device" >&2
          exit 1
        fi
        diskutil erasevolume HFS+ RSScriptRAMDisk "$device" >/dev/null
      fi
      echo "$path"
      ;;
    Linux)
      if [[ ! -d /dev/shm ]]; then
        echo "/dev/shm is not available; RSScript test temp storage requires a ramdisk" >&2
        exit 1
      fi
      echo "/dev/shm/rsscript"
      ;;
    *)
      echo "unsupported OS for automatic RSScript ramdisk: $(uname -s)" >&2
      echo "set RSSCRIPT_RAMDISK_PATH to a mounted ramdisk path" >&2
      exit 1
      ;;
  esac
}

rsscript_configure_ramdisk_paths() {
  local root
  root="$(rsscript_ensure_ramdisk_root)"
  mkdir -p "$root/rsscript-generated-target" "$root/rsscript-temp"
  export RSSCRIPT_RAMDISK_PATH="$root"
  export RSSCRIPT_GENERATED_TARGET_DIR="$root/rsscript-generated-target"
  export RSSCRIPT_TEMP_DIR="$root/rsscript-temp"
}

rsscript_configure_ramdisk_paths
