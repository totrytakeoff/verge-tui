#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./scripts/aur-package.sh [--release] [--deps]

Options:
  --release  Update PKGBUILD sha256sums and regenerate .SRCINFO for AUR publish.
  --deps     Do not pass --nodeps to makepkg.

Default mode (without --release):
  - Build a local source tarball from current workspace
  - Run makepkg with --skipchecksums for quick local test packaging

Release mode (--release):
  - Use PKGBUILD source URL (normally GitHub tag tarball)
  - Refresh sha256sums in PKGBUILD
  - Regenerate .SRCINFO
EOF
}

release_mode=0
nodeps_flag="--nodeps"

while [[ $# -gt 0 ]]; do
  case "$1" in
    --release)
      release_mode=1
      shift
      ;;
    --deps)
      nodeps_flag=""
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
done

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
aur_dir="${repo_root}/aur/verge-tui"
pkgbuild="${aur_dir}/PKGBUILD"

if [[ ! -f "${pkgbuild}" ]]; then
  echo "PKGBUILD not found: ${pkgbuild}" >&2
  exit 1
fi

pkgname="$(awk -F= '/^pkgname=/{gsub(/["'\''[:space:]]/, "", $2); print $2; exit}' "${pkgbuild}")"
pkgver="$(awk -F= '/^pkgver=/{gsub(/["'\''[:space:]]/, "", $2); print $2; exit}' "${pkgbuild}")"

if [[ -z "${pkgname}" || -z "${pkgver}" ]]; then
  echo "Failed to parse pkgname/pkgver from PKGBUILD" >&2
  exit 1
fi

rm -rf "${aur_dir}/src" "${aur_dir}/pkg"

pushd "${aur_dir}" >/dev/null
if [[ "${release_mode}" -eq 1 ]]; then
  rm -f "${pkgname}-${pkgver}.tar.gz"
  sums_line="$(makepkg --geninteg | awk '/^sha256sums=/{print; exit}')"
  if [[ -z "${sums_line}" ]]; then
    echo "Failed to generate sha256sums via makepkg --geninteg" >&2
    exit 1
  fi
  sed -i -E "s|^sha256sums=.*$|${sums_line}|" "${pkgbuild}"
  echo "Updated PKGBUILD: ${sums_line}"

  makepkg -f ${nodeps_flag}
  makepkg --printsrcinfo > .SRCINFO
  echo "Regenerated: ${aur_dir}/.SRCINFO"
else
  tarball="${aur_dir}/${pkgname}-${pkgver}.tar.gz"
  tmpdir="$(mktemp -d)"
  trap 'rm -rf "${tmpdir}"' EXIT
  staged="${tmpdir}/${pkgname}-${pkgver}"
  mkdir -p "${staged}"

  if command -v rsync >/dev/null 2>&1; then
    rsync -a \
      --exclude '.git/' \
      --exclude 'target/' \
      --exclude 'node_modules/' \
      --exclude 'aur/verge-tui/pkg/' \
      --exclude 'aur/verge-tui/src/' \
      --exclude 'aur/verge-tui/*.pkg.tar.*' \
      --exclude 'aur/verge-tui/*.tar.gz' \
      "${repo_root}/" "${staged}/"
  else
    tar -C "${repo_root}" -cf "${tmpdir}/src.tar" \
      --exclude='.git' \
      --exclude='target' \
      --exclude='node_modules' \
      --exclude='aur/verge-tui/pkg' \
      --exclude='aur/verge-tui/src' \
      --exclude='aur/verge-tui/*.pkg.tar.*' \
      --exclude='aur/verge-tui/*.tar.gz' \
      .
    tar -C "${staged}" -xf "${tmpdir}/src.tar"
  fi

  tar -C "${tmpdir}" -czf "${tarball}" "${pkgname}-${pkgver}"
  echo "Created source tarball: ${tarball}"
  makepkg -f ${nodeps_flag} --skipchecksums
fi
popd >/dev/null

echo
echo "Done."
echo "Package files:"
ls -1 "${aur_dir}"/*.pkg.tar.* 2>/dev/null || true
