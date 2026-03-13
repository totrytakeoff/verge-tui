#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
INSTALL_DIR="${ROOT_DIR}/install"
TARGET_DIR="${ROOT_DIR}/target/release"

BUNDLE_CORE=1
CORE_BIN_OVERRIDE="${VERGE_TUI_CORE_BIN:-}"

usage() {
  cat <<'EOF'
Usage: scripts/build-install.sh [options]

Options:
  --out <dir>        Output install directory (default: ./install)
  --no-core          Do not bundle verge-mihomo binary
  --core-bin <path>  Explicit core binary path to bundle
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      INSTALL_DIR="$(realpath -m "${2:-}")"
      shift
      ;;
    --no-core)
      BUNDLE_CORE=0
      ;;
    --core-bin)
      CORE_BIN_OVERRIDE="${2:-}"
      shift
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

resolve_core_bin() {
  if [[ -n "${CORE_BIN_OVERRIDE}" && -x "${CORE_BIN_OVERRIDE}" ]]; then
    echo "${CORE_BIN_OVERRIDE}"
    return 0
  fi

  if command -v verge-mihomo >/dev/null 2>&1; then
    command -v verge-mihomo
    return 0
  fi
  if command -v verge-mihomo-alpha >/dev/null 2>&1; then
    command -v verge-mihomo-alpha
    return 0
  fi
  return 1
}

safe_copy() {
  local src="$1"
  local dst="$2"
  if [[ "$(realpath -m "${src}")" == "$(realpath -m "${dst}")" ]]; then
    return 0
  fi
  cp "${src}" "${dst}"
}

echo "== Build release binary =="
cargo build -p verge-tui --release --manifest-path "${ROOT_DIR}/Cargo.toml"

echo "== Prepare install directory =="
mkdir -p "${INSTALL_DIR}"
rm -rf "${INSTALL_DIR}/bin" "${INSTALL_DIR}/docs"
mkdir -p "${INSTALL_DIR}/bin" "${INSTALL_DIR}/docs"

cp "${TARGET_DIR}/verge-tui" "${INSTALL_DIR}/bin/verge-tui"
cp "${ROOT_DIR}/scripts/proxy-clean-linux.sh" "${INSTALL_DIR}/bin/proxy-clean-linux.sh"
chmod +x "${INSTALL_DIR}/bin/verge-tui" "${INSTALL_DIR}/bin/proxy-clean-linux.sh"

safe_copy "${ROOT_DIR}/install/install.sh" "${INSTALL_DIR}/install.sh"
safe_copy "${ROOT_DIR}/install/uninstall.sh" "${INSTALL_DIR}/uninstall.sh"
safe_copy "${ROOT_DIR}/install/README.md" "${INSTALL_DIR}/README.md"
chmod +x "${INSTALL_DIR}/install.sh" "${INSTALL_DIR}/uninstall.sh"

if [[ "${BUNDLE_CORE}" -eq 1 ]]; then
  if CORE_BIN="$(resolve_core_bin)"; then
    cp "${CORE_BIN}" "${INSTALL_DIR}/bin/$(basename "${CORE_BIN}")"
    chmod +x "${INSTALL_DIR}/bin/$(basename "${CORE_BIN}")"
    echo "Bundled core: ${CORE_BIN}"
  else
    echo "No core binary found. Skip bundling verge-mihomo."
  fi
fi

cp -r "${ROOT_DIR}/docs/." "${INSTALL_DIR}/docs/"
cp "${ROOT_DIR}/README.md" "${INSTALL_DIR}/PROJECT_README.md"
cp "${ROOT_DIR}/LICENSE" "${INSTALL_DIR}/LICENSE"
cp "${ROOT_DIR}/NOTICE.md" "${INSTALL_DIR}/NOTICE.md"

if command -v ldd >/dev/null 2>&1; then
  {
    echo "# Runtime dependencies for verge-tui"
    ldd "${INSTALL_DIR}/bin/verge-tui" || true
  } > "${INSTALL_DIR}/DEPENDENCIES.txt"
fi

if command -v sha256sum >/dev/null 2>&1; then
  (
    cd "${INSTALL_DIR}"
    mapfile -t BIN_FILES < <(find bin -maxdepth 1 -type f | sort)
    sha256sum "${BIN_FILES[@]}" README.md PROJECT_README.md LICENSE NOTICE.md > SHA256SUMS
  ) || true
fi

cat <<EOF
Build completed.

Install package directory:
  ${INSTALL_DIR}

Quick install:
  ${INSTALL_DIR}/install.sh
EOF
