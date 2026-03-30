#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
OUT_DIR="${ROOT_DIR}/dist/deb"
INSTALL_DIR="${ROOT_DIR}/install"
PKGREL="1"
ARCH=""
BUNDLE_CORE=1

usage() {
  cat <<'EOF'
Usage: ./scripts/build-deb.sh [options]

Options:
  --out <dir>        Output directory for .deb (default: ./dist/deb)
  --arch <arch>      Debian arch override (amd64, arm64, all...)
  --pkgrel <rel>     Debian package release suffix (default: 1)
  --no-core          Do not bundle verge-mihomo into the package
  -h, --help         Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --out)
      OUT_DIR="$(realpath -m "${2:-}")"
      shift
      ;;
    --arch)
      ARCH="${2:-}"
      shift
      ;;
    --pkgrel)
      PKGREL="${2:-}"
      shift
      ;;
    --no-core)
      BUNDLE_CORE=0
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

map_arch() {
  case "$(uname -m)" in
    x86_64) echo "amd64" ;;
    aarch64|arm64) echo "arm64" ;;
    armv7l) echo "armhf" ;;
    *) echo "$(uname -m)" ;;
  esac
}

pkg_field() {
  local file="$1"
  local key="$2"
  awk -F= -v key="${key}" '
    {
      left = $1
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", left)
    }
    left == key {
      gsub(/^[[:space:]]+|[[:space:]]+$/, "", $2);
      gsub(/"/, "", $2);
      print $2;
      exit
    }
  ' "${file}"
}

ARCH="${ARCH:-$(map_arch)}"
PKG_NAME="verge-tui"
PKG_VERSION="$(pkg_field "${ROOT_DIR}/apps/verge-tui/Cargo.toml" version)"
DEB_VERSION="${PKG_VERSION}-${PKGREL}"
PKG_FILE="${OUT_DIR}/${PKG_NAME}_${DEB_VERSION}_${ARCH}.deb"

rm -rf "${OUT_DIR}"
mkdir -p "${OUT_DIR}"

build_install_args=(--out "${INSTALL_DIR}")
if [[ "${BUNDLE_CORE}" -eq 0 ]]; then
  build_install_args+=(--no-core)
fi

"${ROOT_DIR}/scripts/build-install.sh" "${build_install_args[@]}"

WORK_DIR="$(mktemp -d)"
trap 'rm -rf "${WORK_DIR}"' EXIT

PKG_ROOT="${WORK_DIR}/pkgroot"
CONTROL_DIR="${PKG_ROOT}/DEBIAN"
DATA_ROOT="${WORK_DIR}/data"

mkdir -p "${CONTROL_DIR}" "${DATA_ROOT}/usr/bin" "${DATA_ROOT}/usr/share/${PKG_NAME}" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}"

install -m 0755 "${INSTALL_DIR}/bin/verge-tui" "${DATA_ROOT}/usr/bin/verge-tui"
if [[ -f "${INSTALL_DIR}/bin/proxy-clean-linux.sh" ]]; then
  install -m 0755 "${INSTALL_DIR}/bin/proxy-clean-linux.sh" "${DATA_ROOT}/usr/bin/verge-tui-proxy-clean"
fi
if [[ -f "${INSTALL_DIR}/bin/verge-mihomo" ]]; then
  install -m 0755 "${INSTALL_DIR}/bin/verge-mihomo" "${DATA_ROOT}/usr/bin/verge-mihomo"
fi
if [[ -f "${INSTALL_DIR}/bin/verge-mihomo-alpha" ]]; then
  install -m 0755 "${INSTALL_DIR}/bin/verge-mihomo-alpha" "${DATA_ROOT}/usr/bin/verge-mihomo-alpha"
fi

for f in README.md PROJECT_README.md LICENSE NOTICE.md DEPENDENCIES.txt SHA256SUMS; do
  if [[ -f "${INSTALL_DIR}/${f}" ]]; then
    install -m 0644 "${INSTALL_DIR}/${f}" "${DATA_ROOT}/usr/share/${PKG_NAME}/${f}"
  fi
done

if [[ -d "${INSTALL_DIR}/docs" ]]; then
  mkdir -p "${DATA_ROOT}/usr/share/${PKG_NAME}/docs"
  cp -a "${INSTALL_DIR}/docs/." "${DATA_ROOT}/usr/share/${PKG_NAME}/docs/"
  if [[ -f "${INSTALL_DIR}/docs/USAGE.md" ]]; then
    install -m 0644 "${INSTALL_DIR}/docs/USAGE.md" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/USAGE.md"
  fi
  if [[ -f "${INSTALL_DIR}/docs/ARCHITECTURE.md" ]]; then
    install -m 0644 "${INSTALL_DIR}/docs/ARCHITECTURE.md" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/ARCHITECTURE.md"
  fi
  if [[ -f "${INSTALL_DIR}/docs/COMMANDS.md" ]]; then
    install -m 0644 "${INSTALL_DIR}/docs/COMMANDS.md" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/COMMANDS.md"
  fi
fi

install -m 0644 "${ROOT_DIR}/README.md" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/README.md"
install -m 0644 "${ROOT_DIR}/NOTICE.md" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/NOTICE.md"
install -m 0644 "${ROOT_DIR}/LICENSE" "${DATA_ROOT}/usr/share/doc/${PKG_NAME}/LICENSE"

cat > "${CONTROL_DIR}/control" <<EOF
Package: ${PKG_NAME}
Version: ${DEB_VERSION}
Section: net
Priority: optional
Architecture: ${ARCH}
Maintainer: totrytakeoff
Depends: libc6, libgcc-s1
Recommends: libcap2-bin
Description: Terminal-first Mihomo/Clash controller
 Standalone Rust TUI for Mihomo/Clash core management.
 Supports subscriptions, node switching, delay test,
 system proxy, TUN management, logs and cleanup.
EOF

(
  cd "${DATA_ROOT}"
  find . -type f -printf '%P\n' | sort | while read -r file; do
    md5sum "${file}"
  done | sed 's#  # #' > "${CONTROL_DIR}/md5sums"
)

chmod 0644 "${CONTROL_DIR}/control" "${CONTROL_DIR}/md5sums"

printf '2.0\n' > "${WORK_DIR}/debian-binary"

(
  cd "${CONTROL_DIR}"
  tar --owner=0 --group=0 --numeric-owner -czf "${WORK_DIR}/control.tar.gz" .
)

(
  cd "${DATA_ROOT}"
  tar --owner=0 --group=0 --numeric-owner -cJf "${WORK_DIR}/data.tar.xz" .
)

rm -f "${PKG_FILE}"
(
  cd "${WORK_DIR}"
  ar rcs "${PKG_FILE}" debian-binary control.tar.gz data.tar.xz
)

echo "Built .deb:"
echo "  ${PKG_FILE}"
