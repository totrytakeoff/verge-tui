#!/usr/bin/env bash
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
DEB_DIR="${ROOT_DIR}/dist/deb"
REPO_DIR="${ROOT_DIR}/dist/apt"
ORIGIN="verge-tui"
LABEL="verge-tui APT Repository"
SUITE="stable"
CODENAME="stable"
COMPONENT="main"
GPG_KEY=""
PUBLIC_KEY_NAME="verge-tui-archive-keyring"

usage() {
  cat <<'EOF'
Usage: ./scripts/build-apt-repo.sh [options]

Options:
  --deb-dir <dir>     Input directory containing .deb packages (default: ./dist/deb)
  --repo-dir <dir>    Output repository directory (default: ./dist/apt)
  --origin <name>     Release Origin field
  --label <name>      Release Label field
  --suite <name>      Release Suite field (default: stable)
  --codename <name>   Release Codename field (default: stable)
  --component <name>  Repository component (default: main)
  --gpg-key <id>      Sign Release/InRelease using this GPG key
  --public-key-name <name>
                      Base name for exported public key files
  -h, --help          Show this help
EOF
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --deb-dir)
      DEB_DIR="$(realpath -m "${2:-}")"
      shift
      ;;
    --repo-dir)
      REPO_DIR="$(realpath -m "${2:-}")"
      shift
      ;;
    --origin)
      ORIGIN="${2:-}"
      shift
      ;;
    --label)
      LABEL="${2:-}"
      shift
      ;;
    --suite)
      SUITE="${2:-}"
      shift
      ;;
    --codename)
      CODENAME="${2:-}"
      shift
      ;;
    --component)
      COMPONENT="${2:-}"
      shift
      ;;
    --gpg-key)
      GPG_KEY="${2:-}"
      shift
      ;;
    --public-key-name)
      PUBLIC_KEY_NAME="${2:-}"
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

if ! compgen -G "${DEB_DIR}/*.deb" > /dev/null; then
  echo "No .deb packages found in ${DEB_DIR}" >&2
  exit 1
fi

extract_control() {
  local deb="$1"
  local work="$2"
  rm -rf "${work}"
  mkdir -p "${work}"
  (
    cd "${work}"
    ar x "${deb}" >/dev/null
    if [[ -f control.tar.xz ]]; then
      tar -xJf control.tar.xz ./control >/dev/null 2>&1
    elif [[ -f control.tar.gz ]]; then
      tar -xzf control.tar.gz ./control >/dev/null 2>&1
    else
      echo "Unsupported control archive in ${deb}" >&2
      exit 1
    fi
  )
}

field_from_control() {
  local control="$1"
  local key="$2"
  awk -F': ' -v key="${key}" '$1 == key {print substr($0, length($1)+3); exit}' "${control}"
}

package_entry() {
  local deb="$1"
  local repo_rel_dir="$2"
  local tmp_dir
  tmp_dir="$(mktemp -d)"
  extract_control "${deb}" "${tmp_dir}"
  local control="${tmp_dir}/control"
  local filename="${repo_rel_dir}/$(basename "${deb}")"
  local size md5 sha1 sha256
  size="$(stat -c '%s' "${deb}")"
  md5="$(md5sum "${deb}" | awk '{print $1}')"
  sha1="$(sha1sum "${deb}" | awk '{print $1}')"
  sha256="$(sha256sum "${deb}" | awk '{print $1}')"

  cat <<EOF
Package: $(field_from_control "${control}" "Package")
Version: $(field_from_control "${control}" "Version")
Architecture: $(field_from_control "${control}" "Architecture")
Maintainer: $(field_from_control "${control}" "Maintainer")
Depends: $(field_from_control "${control}" "Depends")
Recommends: $(field_from_control "${control}" "Recommends")
Section: $(field_from_control "${control}" "Section")
Priority: $(field_from_control "${control}" "Priority")
Filename: ${filename}
Size: ${size}
MD5sum: ${md5}
SHA1: ${sha1}
SHA256: ${sha256}
Description: $(field_from_control "${control}" "Description")

EOF
  rm -rf "${tmp_dir}"
}

release_hash_block() {
  local algo="$1"
  local sum_cmd="$2"
  shift 2
  echo "${algo}:"
  for file in "$@"; do
    local rel size sum
    rel="${file#${REPO_DIR}/}"
    size="$(stat -c '%s' "${file}")"
    sum="$(${sum_cmd} "${file}" | awk '{print $1}')"
    printf " %s %16s %s\n" "${sum}" "${size}" "${rel}"
  done
}

rm -rf "${REPO_DIR}"
mkdir -p "${REPO_DIR}/pool/${COMPONENT}/v/verge-tui"

for deb in "${DEB_DIR}"/*.deb; do
  cp "${deb}" "${REPO_DIR}/pool/${COMPONENT}/v/verge-tui/"
done

declare -A ARCH_SEEN=()
for deb in "${REPO_DIR}/pool/${COMPONENT}/v/verge-tui/"*.deb; do
  arch="$(basename "${deb}" .deb | awk -F_ '{print $NF}')"
  ARCH_SEEN["${arch}"]=1
done

release_files=()
for arch in "${!ARCH_SEEN[@]}"; do
  binary_dir="${REPO_DIR}/dists/${CODENAME}/${COMPONENT}/binary-${arch}"
  mkdir -p "${binary_dir}"
  packages_file="${binary_dir}/Packages"
  : > "${packages_file}"
  for deb in "${REPO_DIR}/pool/${COMPONENT}/v/verge-tui/"*.deb; do
    deb_arch="$(basename "${deb}" .deb | awk -F_ '{print $NF}')"
    if [[ "${deb_arch}" == "${arch}" ]]; then
      package_entry "${deb}" "pool/${COMPONENT}/v/verge-tui" >> "${packages_file}"
    fi
  done
  gzip -kf "${packages_file}"
  release_files+=("${packages_file}" "${packages_file}.gz")
done

release_file="${REPO_DIR}/dists/${CODENAME}/Release"
mkdir -p "$(dirname "${release_file}")"
{
  echo "Origin: ${ORIGIN}"
  echo "Label: ${LABEL}"
  echo "Suite: ${SUITE}"
  echo "Codename: ${CODENAME}"
  echo "Architectures: ${!ARCH_SEEN[*]}"
  echo "Components: ${COMPONENT}"
  echo "Date: $(LC_ALL=C date -Ru)"
  release_hash_block "MD5Sum" md5sum "${release_files[@]}"
  release_hash_block "SHA256" sha256sum "${release_files[@]}"
} > "${release_file}"

if [[ -n "${GPG_KEY}" ]]; then
  gpg_sign_args=(--batch --yes --local-user "${GPG_KEY}")
  gpg_export_args=(--batch --yes)
  if [[ -n "${GPG_PASSPHRASE:-}" ]]; then
    gpg_sign_args+=(--pinentry-mode loopback --passphrase "${GPG_PASSPHRASE}")
    gpg_export_args+=(--pinentry-mode loopback --passphrase "${GPG_PASSPHRASE}")
  fi

  gpg "${gpg_sign_args[@]}" --armor --detach-sign --output "${release_file}.gpg" "${release_file}"
  gpg "${gpg_sign_args[@]}" --clearsign --output "${REPO_DIR}/dists/${CODENAME}/InRelease" "${release_file}"
  gpg "${gpg_export_args[@]}" --armor --export "${GPG_KEY}" > "${REPO_DIR}/${PUBLIC_KEY_NAME}.asc"
  gpg "${gpg_export_args[@]}" --export "${GPG_KEY}" | gpg --dearmor > "${REPO_DIR}/${PUBLIC_KEY_NAME}.gpg"
fi

echo "Built APT repository:"
echo "  ${REPO_DIR}"
