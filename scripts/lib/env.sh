#!/usr/bin/env bash

has_required_capabilities() {
  local hexadecimal
  hexadecimal=$(awk '/^CapEff:/ { print $2; exit }' /proc/self/status)
  [[ -n $hexadecimal ]] || return 1
  local capabilities=$((16#$hexadecimal))
  (( capabilities & (1 << 12) )) && (( capabilities & (1 << 21) )) && (( capabilities & (1 << 39) ))
}

validate_environment() {
  [[ $(uname -s) == Linux ]] || die "privileged acceptance requires Linux"
  has_required_capabilities || die "privileged acceptance requires CAP_NET_ADMIN, CAP_SYS_ADMIN, and CAP_BPF; root without them is insufficient"
  [[ -r /etc/os-release ]] || die "cannot read /etc/os-release"
  # shellcheck disable=SC1091
  . /etc/os-release
  [[ ${ID:-} == ubuntu && ${VERSION_ID:-} == 24.04 ]] || die "requires Ubuntu 24.04; found ${PRETTY_NAME:-unknown}"

  local kernel major minor
  IFS=. read -r major minor _ <<<"$(uname -r)"
  (( major > 6 || (major == 6 && minor >= 8) )) || die "requires Linux >= 6.8; found $(uname -r)"
  [[ -f /sys/fs/cgroup/cgroup.controllers ]] || die "cgroup v2 unified hierarchy missing"
  [[ -r /sys/kernel/btf/vmlinux ]] || die "kernel BTF missing: /sys/kernel/btf/vmlinux"
  mountpoint -q /sys/fs/bpf || die "bpffs is not mounted at /sys/fs/bpf"
  grep -qs ' /sys/fs/bpf bpf ' /proc/mounts || die "/sys/fs/bpf is not a bpffs mount"

  local binary
  for binary in cargo python3 bpf-linker bpftool find mountpoint; do
    require_command "$binary"
  done
}

current_cgroup_path() {
  local relative
  relative=$(awk -F: '$1 == "0" { print $3; exit }' /proc/self/cgroup)
  [[ -n $relative ]] || return 1
  printf '/sys/fs/cgroup%s\n' "$relative"
}
