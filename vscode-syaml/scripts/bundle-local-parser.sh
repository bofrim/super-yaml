#!/usr/bin/env bash

set -euo pipefail

script_dir="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
extension_dir="$(cd "${script_dir}/.." && pwd)"
repo_root="$(cd "${extension_dir}/.." && pwd)"

source_binary="${1:-${repo_root}/target/debug/super-yaml}"
if [[ ! -f "${source_binary}" ]]; then
  echo "Parser binary not found: ${source_binary}" >&2
  echo "Build it first with: cargo build --bin super-yaml" >&2
  exit 1
fi

platform="$(uname -s | tr '[:upper:]' '[:lower:]')"
arch_raw="$(uname -m)"
case "${arch_raw}" in
  x86_64)
    arch="x64"
    ;;
  arm64|aarch64)
    arch="arm64"
    ;;
  *)
    arch="${arch_raw}"
    ;;
esac

dest_dir="${extension_dir}/bin/${platform}-${arch}"
dest_binary="${dest_dir}/super-yaml"
mkdir -p "${dest_dir}"
cp "${source_binary}" "${dest_binary}"
chmod 755 "${dest_binary}"

echo "Bundled parser: ${dest_binary}"
