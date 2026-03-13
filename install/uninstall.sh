#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr/local"
USER_MODE=0

usage() {
  cat <<'EOF'
Usage: uninstall.sh [options]

Options:
  --prefix <path>  Install prefix (default: /usr/local)
  --user           Use ~/.local as install prefix
  -h, --help       Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --prefix)
      PREFIX="${2:-}"
      shift
      ;;
    --user)
      USER_MODE=1
      PREFIX="${HOME}/.local"
      ;;
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

SUDO=()
if [[ "${USER_MODE}" -eq 0 && "${EUID}" -ne 0 ]]; then
  NEED_SUDO=0
  if [[ -e "${PREFIX}" ]]; then
    if [[ ! -w "${PREFIX}" ]]; then
      NEED_SUDO=1
    fi
  else
    if [[ ! -w "$(dirname "${PREFIX}")" ]]; then
      NEED_SUDO=1
    fi
  fi
  if [[ "${NEED_SUDO}" -eq 1 ]]; then
    SUDO=(sudo)
  fi
fi

BINDIR="${PREFIX}/bin"
SHAREDIR="${PREFIX}/share/verge-tui"

"${SUDO[@]}" rm -f \
  "${BINDIR}/verge-tui" \
  "${BINDIR}/verge-tui-proxy-clean" \
  "${BINDIR}/verge-mihomo" \
  "${BINDIR}/verge-mihomo-alpha"
"${SUDO[@]}" rm -rf "${SHAREDIR}"

echo "Uninstalled verge-tui from ${PREFIX}"
