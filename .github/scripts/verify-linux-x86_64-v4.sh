#!/usr/bin/env bash
set -euo pipefail

required_v4_flags=(
  cx16
  lahf_lm
  popcnt
  sse4_1
  sse4_2
  ssse3
  avx
  avx2
  bmi1
  bmi2
  f16c
  fma
  lzcnt
  movbe
  osxsave
  xsave
  avx512f
  avx512bw
  avx512cd
  avx512dq
  avx512vl
)

has_cpu_flag() {
  local flags="$1"
  local flag="$2"

  if [[ " $flags " == *" $flag "* ]]; then
    return 0
  fi

  if [[ "$flag" == "lzcnt" && " $flags " == *" abm "* ]]; then
    return 0
  fi

  return 1
}

cpu_supports_x86_64_v4() {
  if [[ "$(uname -s)" != "Linux" || "$(uname -m)" != "x86_64" ]]; then
    echo "host is not Linux x86_64"
    return 1
  fi

  local flags
  flags="$(grep -m1 -E '^(flags|Features)[[:space:]]*:' /proc/cpuinfo || true)"
  if [[ -z "$flags" ]]; then
    echo "unable to read CPU flags from /proc/cpuinfo"
    return 1
  fi

  local missing=()
  for flag in "${required_v4_flags[@]}"; do
    if ! has_cpu_flag "$flags" "$flag"; then
      missing+=("$flag")
    fi
  done

  if (( ${#missing[@]} > 0 )); then
    echo "host CPU is missing x86-64-v4 flags: ${missing[*]}"
    return 1
  fi

  echo "host CPU advertises x86-64-v4 runtime support"
}

if [[ "${1:-}" == "--cpu-only" ]]; then
  cpu_supports_x86_64_v4
  exit $?
fi

binary="${1:-target/release/nx86-app}"

if [[ ! -f "$binary" ]]; then
  echo "::error::expected binary was not found: $binary"
  exit 1
fi

if [[ ! -x "$binary" ]]; then
  echo "::error::expected binary is not executable: $binary"
  exit 1
fi

echo "::group::ELF identity"
file "$binary"
file_output="$(file -b "$binary")"
if [[ "$file_output" != *"ELF 64-bit"* || "$file_output" != *"x86-64"* ]]; then
  echo "::error::artifact is not an ELF64 x86-64 binary"
  exit 1
fi

machine="$(readelf -h "$binary" | awk -F: '/Machine:/ {gsub(/^[ \t]+/, "", $2); print $2}')"
if [[ "$machine" != *"X86-64"* ]]; then
  echo "::error::unexpected ELF machine: $machine"
  exit 1
fi
readelf -h "$binary"
echo "::endgroup::"

echo "::group::Dynamic linking"
if readelf -l "$binary" | grep -q 'Requesting program interpreter'; then
  echo "::warning::binary has a dynamic ELF interpreter; this is allowed by the pragmatic GUI artifact profile"
  readelf -l "$binary" | grep 'Requesting program interpreter'
else
  echo "binary has no dynamic ELF interpreter"
fi

if readelf -d "$binary"; then
  true
else
  echo "binary has no dynamic section"
fi

if ldd_output="$(ldd "$binary" 2>&1)"; then
  printf '%s\n' "$ldd_output"
else
  ldd_status=$?
  printf '%s\n' "$ldd_output"
  if [[ "$ldd_output" != *"not a dynamic executable"* ]]; then
    echo "::warning::ldd exited with status $ldd_status"
  fi
fi
echo "::endgroup::"

echo "::group::Worker smoke"
if cpu_supports_x86_64_v4; then
  compiler_log="$(mktemp)"
  runtime_log="$(mktemp)"
  trap 'rm -f "$compiler_log" "$runtime_log"' EXIT

  "$binary" --worker compiler-smoke >"$compiler_log"
  "$binary" --worker runtime-smoke >"$runtime_log"

  echo "compiler-smoke output:"
  tail -n 20 "$compiler_log"
  echo "runtime-smoke output:"
  tail -n 20 "$runtime_log"
else
  echo "::warning::skipping worker smoke because this runner cannot execute x86-64-v4 code"
fi
echo "::endgroup::"
