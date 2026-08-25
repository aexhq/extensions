#!/bin/sh
set -eu

expect() {
    actual=$1
    expected=$2
    label=$3
    if [ "$actual" != "$expected" ]; then
        printf '%s: expected %s, got %s\n' "$label" "$expected" "$actual" >&2
        exit 1
    fi
}

expect "$(id -u)" 1001 uid
expect "$(id -g)" 1001 gid
# 0xe0 = CAP_KILL (bit 5) | CAP_SETGID (bit 6) | CAP_SETUID (bit 7): exactly the supervisor set.
expected_capabilities=00000000000000e0
for field in CapInh CapPrm CapEff CapAmb; do
    actual=$(awk -v field="$field:" '$1 == field { print $2 }' /proc/self/status)
    expect "$actual" "$expected_capabilities" "$field"
done
printf 'supervisor-launched\n'
