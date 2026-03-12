#!/usr/bin/env bash
set -euo pipefail

# Clean up proxy/TUN leftovers on Linux so another proxy client can start cleanly.
# Default mode targets clash-verge/mihomo related leftovers only.
#
# Usage:
#   ./scripts/proxy-clean-linux.sh
#   ./scripts/proxy-clean-linux.sh --dry-run
#   ./scripts/proxy-clean-linux.sh --yes
#   ./scripts/proxy-clean-linux.sh --aggressive --yes

DRY_RUN=0
ASSUME_YES=0
AGGRESSIVE=0

usage() {
  cat <<'EOF'
Usage: proxy-clean-linux.sh [options]

Options:
  --dry-run      Print actions only, no changes.
  --yes          Skip interactive confirmation.
  --aggressive   Also clean common non-clash proxy leftovers (sing-box/xray/v2ray).
  -h, --help     Show this help.
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) DRY_RUN=1 ;;
    --yes|-y) ASSUME_YES=1 ;;
    --aggressive) AGGRESSIVE=1 ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "Unknown option: $1" >&2
      usage
      exit 1
      ;;
  esac
  shift
done

if [[ "$(uname -s)" != "Linux" ]]; then
  echo "This script currently supports Linux only."
  exit 1
fi

SUDO_BIN=()
if [[ "${EUID}" -ne 0 ]]; then
  SUDO_BIN=(sudo)
  if [[ "${DRY_RUN}" -eq 0 ]] && ! command -v sudo >/dev/null 2>&1; then
    echo "sudo is required (or run as root)." >&2
    exit 1
  fi
fi

print_cmd() {
  printf '+'
  for arg in "$@"; do
    printf ' %q' "${arg}"
  done
  printf '\n'
}

run_cmd() {
  print_cmd "$@"
  if [[ "${DRY_RUN}" -eq 0 ]]; then
    "$@"
  fi
}

run_root_cmd() {
  run_cmd "${SUDO_BIN[@]}" "$@"
}

kill_by_name() {
  local signal="$1"
  local name="$2"

  # Linux process "comm" may be truncated to 15 chars; use -f for long names.
  if [[ "${#name}" -gt 15 ]]; then
    # Match path or bare command token boundary to avoid broad accidental matches.
    run_root_cmd pkill "-${signal}" -f "(^|[[:space:]/])${name}([[:space:]]|$)" || true
  else
    run_root_cmd pkill "-${signal}" -x "${name}" || true
  fi
}

has_cmd() {
  command -v "$1" >/dev/null 2>&1
}

log_section() {
  printf '\n== %s ==\n' "$1"
}

list_processes() {
  ps -ef | grep -Ei 'clash-verge|verge-mihomo|mihomo|sing-box|xray|v2ray' | grep -v grep || true
}

list_tun_ifaces() {
  ip -brief link 2>/dev/null | grep -Ei 'tun|tap|verge|mihomo|clash|sing|xray|v2ray' || true
}

list_proxy_rules() {
  "${SUDO_BIN[@]}" ip -4 rule show 2>/dev/null | grep -Ei 'fwmark|lookup|table' || true
  "${SUDO_BIN[@]}" ip -6 rule show 2>/dev/null | grep -Ei 'fwmark|lookup|table' || true
}

list_proxy_tables() {
  "${SUDO_BIN[@]}" ip -4 route show table all 2>/dev/null | grep -Ei 'table|tun|clash|mihomo|verge|sing|xray|v2ray' || true
  "${SUDO_BIN[@]}" ip -6 route show table all 2>/dev/null | grep -Ei 'table|tun|clash|mihomo|verge|sing|xray|v2ray' || true
}

if [[ "${DRY_RUN}" -eq 0 && "${#SUDO_BIN[@]}" -gt 0 ]]; then
  log_section "Authenticate sudo"
  run_cmd sudo -v
fi

log_section "Pre-check / Processes"
list_processes
log_section "Pre-check / TUN-ish Interfaces"
list_tun_ifaces
log_section "Pre-check / IP Rules"
list_proxy_rules

if [[ "${ASSUME_YES}" -eq 0 ]]; then
  echo
  read -r -p "Proceed with cleanup? [y/N] " answer
  if [[ ! "${answer}" =~ ^[Yy]$ ]]; then
    echo "Aborted."
    exit 0
  fi
fi

log_section "Stop clash-verge services (best effort)"
if has_cmd systemctl; then
  run_root_cmd systemctl stop clash-verge-service.service || true
  run_root_cmd systemctl stop clash-verge.service || true
fi

log_section "Stop proxy processes"
PROC_NAMES=(
  clash-verge
  clash-verge-service
  verge-mihomo
  verge-mihomo-alpha
)
if [[ "${AGGRESSIVE}" -eq 1 ]]; then
  PROC_NAMES+=(sing-box singbox xray xray-core v2ray v2raya)
fi

for sig in TERM KILL; do
  for name in "${PROC_NAMES[@]}"; do
    kill_by_name "${sig}" "${name}"
  done
  [[ "${sig}" == "TERM" ]] && sleep 1
done

log_section "Remove known socket leftovers"
SOCKET_PATHS=(
  /tmp/verge/verge-mihomo.sock
  /var/tmp/verge/verge-mihomo.sock
  /tmp/verge/clash-verge-service.sock
)
for p in "${SOCKET_PATHS[@]}"; do
  run_root_cmd rm -f "${p}" || true
done
run_root_cmd rmdir /tmp/verge 2>/dev/null || true
run_root_cmd rmdir /var/tmp/verge 2>/dev/null || true

log_section "Detect target interfaces"
IFACE_REGEX='(verge|mihomo|clash)'
if [[ "${AGGRESSIVE}" -eq 1 ]]; then
  IFACE_REGEX='(verge|mihomo|clash|sing|xray|v2ray|^tun[0-9]+$|^tap[0-9]+$|^utun[0-9]+$)'
fi

mapfile -t TARGET_IFACES < <(
  "${SUDO_BIN[@]}" ip -o link show 2>/dev/null \
    | awk -F': ' '{print $2}' \
    | sed 's/@.*//' \
    | grep -Eiv '^lo$' \
    | grep -Ei "${IFACE_REGEX}" \
    | sort -u || true
)

if [[ "${#TARGET_IFACES[@]}" -eq 0 ]]; then
  echo "No matching proxy interfaces found."
else
  printf 'Interfaces: %s\n' "${TARGET_IFACES[*]}"
fi

log_section "Delete target interfaces"
for iface in "${TARGET_IFACES[@]}"; do
  run_root_cmd ip link set dev "${iface}" down || true
  run_root_cmd ip link delete dev "${iface}" || true
done

log_section "Collect affected route tables"
declare -A TABLE_SET=()

for iface in "${TARGET_IFACES[@]}"; do
  while read -r tbl; do
    [[ -z "${tbl}" ]] && continue
    case "${tbl}" in
      main|local|default|unspec|all) continue ;;
    esac
    TABLE_SET["${tbl}"]=1
  done < <(
    "${SUDO_BIN[@]}" ip -4 route show table all 2>/dev/null \
      | awk -v d="${iface}" '
          $0 ~ (" dev " d "([ ]|$)") {
            for (i = 1; i <= NF; i++) {
              if ($i == "table" && (i + 1) <= NF) print $(i + 1)
            }
          }'
  )

  while read -r tbl; do
    [[ -z "${tbl}" ]] && continue
    case "${tbl}" in
      main|local|default|unspec|all) continue ;;
    esac
    TABLE_SET["${tbl}"]=1
  done < <(
    "${SUDO_BIN[@]}" ip -6 route show table all 2>/dev/null \
      | awk -v d="${iface}" '
          $0 ~ (" dev " d "([ ]|$)") {
            for (i = 1; i <= NF; i++) {
              if ($i == "table" && (i + 1) <= NF) print $(i + 1)
            }
          }'
  )
done

mapfile -t TABLES < <(printf '%s\n' "${!TABLE_SET[@]}" | sort -u)
if [[ "${#TABLE_SET[@]}" -eq 0 ]]; then
  TABLES=()
fi

if [[ "${AGGRESSIVE}" -eq 1 ]]; then
  while read -r tbl; do
    [[ -z "${tbl}" ]] && continue
    case "${tbl}" in
      main|local|default|unspec|all) continue ;;
    esac
    TABLE_SET["${tbl}"]=1
  done < <(
    "${SUDO_BIN[@]}" ip -4 rule show 2>/dev/null \
      | awk '/fwmark/ {
          for (i = 1; i <= NF; i++) {
            if (($i == "lookup" || $i == "table") && (i + 1) <= NF) print $(i + 1)
          }
        }'
  )

  while read -r tbl; do
    [[ -z "${tbl}" ]] && continue
    case "${tbl}" in
      main|local|default|unspec|all) continue ;;
    esac
    TABLE_SET["${tbl}"]=1
  done < <(
    "${SUDO_BIN[@]}" ip -6 rule show 2>/dev/null \
      | awk '/fwmark/ {
          for (i = 1; i <= NF; i++) {
            if (($i == "lookup" || $i == "table") && (i + 1) <= NF) print $(i + 1)
          }
        }'
  )

  mapfile -t TABLES < <(printf '%s\n' "${!TABLE_SET[@]}" | sort -u)
fi

if [[ "${#TABLES[@]}" -eq 0 ]]; then
  echo "No affected custom route tables found."
else
  printf 'Route tables: %s\n' "${TABLES[*]}"
fi

delete_rules_for_table() {
  local family="$1"
  local table="$2"
  mapfile -t prefs < <(
    "${SUDO_BIN[@]}" ip "${family}" rule show 2>/dev/null \
      | awk -v t="${table}" '
          ($0 ~ (" lookup " t "($| )")) || ($0 ~ (" table " t "($| )")) {
            p=$1
            sub(":", "", p)
            print p
          }' \
      | sort -rn -u
  )

  for pref in "${prefs[@]}"; do
    run_root_cmd ip "${family}" rule del pref "${pref}" || true
  done
}

log_section "Clean rules/routes for affected tables"
for tbl in "${TABLES[@]}"; do
  delete_rules_for_table -4 "${tbl}"
  delete_rules_for_table -6 "${tbl}"
  run_root_cmd ip -4 route flush table "${tbl}" || true
  run_root_cmd ip -6 route flush table "${tbl}" || true
done

NFT_REGEX='(clash|verge|mihomo)'
if [[ "${AGGRESSIVE}" -eq 1 ]]; then
  NFT_REGEX='(clash|verge|mihomo|sing|xray|v2ray)'
fi

log_section "Clean nft tables (pattern-based)"
if has_cmd nft; then
  mapfile -t NFT_TABLES < <(
    "${SUDO_BIN[@]}" nft list tables 2>/dev/null \
      | awk '/^table / {print $2" "$3}' \
      | grep -Ei "${NFT_REGEX}" || true
  )

  for row in "${NFT_TABLES[@]}"; do
    family="${row%% *}"
    name="${row##* }"
    run_root_cmd nft delete table "${family}" "${name}" || true
  done
else
  echo "nft not found, skip."
fi

log_section "Post-check / Processes"
list_processes
log_section "Post-check / TUN-ish Interfaces"
list_tun_ifaces
log_section "Post-check / IP Rules"
list_proxy_rules
log_section "Post-check / Route Tables"
list_proxy_tables

echo
echo "Cleanup completed."
if [[ "${AGGRESSIVE}" -eq 0 ]]; then
  echo "Tip: if another proxy still cannot start TUN, re-run with --aggressive."
fi
