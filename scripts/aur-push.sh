#!/usr/bin/env bash
set -euo pipefail

usage() {
  cat <<'EOF'
Usage:
  ./scripts/aur-push.sh [options]

Options:
  --no-package         Skip running ./scripts/aur-package.sh --release first.
  --deps               Pass --deps to aur-package.sh when packaging.
  --message <msg>      Commit message for AUR update.
  --aur-dir <path>     Local AUR metadata dir. Default: ./aur/verge-tui
  --repo-url <url>     AUR git URL. Default: ssh://aur@aur.archlinux.org/<pkgname>.git
  --branch <name>      Push branch. Default: master
  --dry-run            Commit locally but do not push.
  -h, --help           Show this help.

Examples:
  ./scripts/aur-push.sh
  ./scripts/aur-push.sh --no-package --message "verge-tui 0.1.1-1"
  ./scripts/aur-push.sh --dry-run
EOF
}

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
aur_dir="${repo_root}/aur/verge-tui"
branch="master"
run_package=1
pass_deps=0
dry_run=0
commit_message=""
repo_url=""

while [[ $# -gt 0 ]]; do
  case "$1" in
    --no-package)
      run_package=0
      shift
      ;;
    --deps)
      pass_deps=1
      shift
      ;;
    --message)
      commit_message="${2:-}"
      shift 2
      ;;
    --aur-dir)
      aur_dir="${2:-}"
      shift 2
      ;;
    --repo-url)
      repo_url="${2:-}"
      shift 2
      ;;
    --branch)
      branch="${2:-}"
      shift 2
      ;;
    --dry-run)
      dry_run=1
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

pkgbuild="${aur_dir}/PKGBUILD"
srcinfo="${aur_dir}/.SRCINFO"

if [[ ! -f "${pkgbuild}" ]]; then
  echo "PKGBUILD not found: ${pkgbuild}" >&2
  exit 1
fi

if [[ "${run_package}" -eq 1 ]]; then
  package_args=(--release)
  if [[ "${pass_deps}" -eq 1 ]]; then
    package_args+=(--deps)
  fi
  echo "==> Packaging AUR metadata via aur-package.sh ${package_args[*]}"
  "${repo_root}/scripts/aur-package.sh" "${package_args[@]}"
fi

if [[ ! -f "${srcinfo}" ]]; then
  echo ".SRCINFO not found: ${srcinfo}" >&2
  echo "Run ./scripts/aur-package.sh --release first." >&2
  exit 1
fi

pkgname="$(awk -F= '/^pkgname=/{gsub(/["'\''[:space:]]/, "", $2); print $2; exit}' "${pkgbuild}")"
pkgver="$(awk -F= '/^pkgver=/{gsub(/["'\''[:space:]]/, "", $2); print $2; exit}' "${pkgbuild}")"
pkgrel="$(awk -F= '/^pkgrel=/{gsub(/["'\''[:space:]]/, "", $2); print $2; exit}' "${pkgbuild}")"

if [[ -z "${pkgname}" ]]; then
  echo "Failed to parse pkgname from ${pkgbuild}" >&2
  exit 1
fi

if [[ -z "${repo_url}" ]]; then
  repo_url="ssh://aur@aur.archlinux.org/${pkgname}.git"
fi

if [[ -z "${commit_message}" ]]; then
  commit_message="${pkgname} ${pkgver}-${pkgrel}"
fi

workdir="$(mktemp -d)"
trap 'rm -rf "${workdir}"' EXIT
clone_dir="${workdir}/${pkgname}-aur"

echo "==> Cloning AUR repo: ${repo_url}"
git clone "${repo_url}" "${clone_dir}"

echo "==> Syncing PKGBUILD/.SRCINFO"
cp "${pkgbuild}" "${clone_dir}/PKGBUILD"
cp "${srcinfo}" "${clone_dir}/.SRCINFO"
if [[ -f "${aur_dir}/.gitignore" ]]; then
  cp "${aur_dir}/.gitignore" "${clone_dir}/.gitignore"
fi

pushd "${clone_dir}" >/dev/null
git add PKGBUILD .SRCINFO
if [[ -f .gitignore ]]; then
  git add .gitignore
fi

if git diff --cached --quiet; then
  echo "No AUR metadata changes detected. Nothing to push."
  popd >/dev/null
  exit 0
fi

git commit -m "${commit_message}"

if [[ "${dry_run}" -eq 1 ]]; then
  echo "Dry-run enabled: commit created locally but not pushed."
  echo "Local clone: ${clone_dir}"
else
  echo "==> Pushing to ${repo_url} (${branch})"
  git push origin "HEAD:${branch}"
  popd >/dev/null
  echo "Done: pushed ${pkgname} to AUR."
  exit 0
fi
popd >/dev/null
