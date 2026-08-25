#!/bin/sh
set -eu

test "$(id -u)" = 1001
test "$(id -g)" = 1001
# 0xe0 = CAP_KILL (bit 5) | CAP_SETGID (bit 6) | CAP_SETUID (bit 7): exactly the supervisor set.
expected_capabilities=00000000000000e0
for field in CapInh CapPrm CapEff CapAmb; do
    actual=$(awk -v field="$field:" '$1 == field { print $2 }' /proc/self/status)
    test "$actual" = "$expected_capabilities"
done
printf 'supervisor-launched\n'
