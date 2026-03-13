#!/usr/bin/env bash
set -euo pipefail

PREFIX="/usr/local"
USER_MODE=0

usage() {
  cat <<'EOF'
Usage: install.sh [options]

Options:
  --prefix <path>  Install prefix (default: /usr/local)
  --user           Install to ~/.local
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

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PKG_BIN_DIR="${SCRIPT_DIR}/bin"
PKG_DOC_DIR="${SCRIPT_DIR}/docs"

if [[ ! -x "${PKG_BIN_DIR}/verge-tui" ]]; then
  echo "Missing ${PKG_BIN_DIR}/verge-tui" >&2
  echo "Run ./scripts/build-install.sh first." >&2
  exit 1
fi

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

echo "Install prefix: ${PREFIX}"
"${SUDO[@]}" install -d "${BINDIR}" "${SHAREDIR}"

"${SUDO[@]}" install -m 0755 "${PKG_BIN_DIR}/verge-tui" "${BINDIR}/verge-tui"
if [[ -f "${PKG_BIN_DIR}/proxy-clean-linux.sh" ]]; then
  "${SUDO[@]}" install -m 0755 "${PKG_BIN_DIR}/proxy-clean-linux.sh" "${BINDIR}/verge-tui-proxy-clean"
fi
if [[ -f "${PKG_BIN_DIR}/verge-mihomo" ]]; then
  "${SUDO[@]}" install -m 0755 "${PKG_BIN_DIR}/verge-mihomo" "${BINDIR}/verge-mihomo"
fi
if [[ -f "${PKG_BIN_DIR}/verge-mihomo-alpha" ]]; then
  "${SUDO[@]}" install -m 0755 "${PKG_BIN_DIR}/verge-mihomo-alpha" "${BINDIR}/verge-mihomo-alpha"
fi

for f in README.md PROJECT_README.md LICENSE NOTICE.md DEPENDENCIES.txt SHA256SUMS; do
  if [[ -f "${SCRIPT_DIR}/${f}" ]]; then
    "${SUDO[@]}" install -m 0644 "${SCRIPT_DIR}/${f}" "${SHAREDIR}/${f}"
  fi
done

if [[ -d "${PKG_DOC_DIR}" ]]; then
  "${SUDO[@]}" rm -rf "${SHAREDIR}/docs"
  "${SUDO[@]}" mkdir -p "${SHAREDIR}/docs"
  "${SUDO[@]}" cp -a "${PKG_DOC_DIR}/." "${SHAREDIR}/docs/"
fi

cat <<EOF
Installed:
  ${BINDIR}/verge-tui
  ${BINDIR}/verge-tui-proxy-clean
  ${SHAREDIR}

Run:
  verge-tui
EOF
