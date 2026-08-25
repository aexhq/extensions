#!/bin/sh
# Root boot boundary for the trusted Environment supervisor and every Tool uid.
set -eu

# Keep the provider endpoint privileged. The supervisor receives CAP_NET_BIND_SERVICE; Tool
# children receive no capabilities and therefore cannot impersonate the endpoint after a crash.
unprivileged_port_start=$(( 8080 + 1 ))
if [ "$(cat /proc/sys/net/ipv4/ip_unprivileged_port_start)" != "$unprivileged_port_start" ]; then
  printf '%s\n' "$unprivileged_port_start" > /proc/sys/net/ipv4/ip_unprivileged_port_start
fi
test "$(cat /proc/sys/net/ipv4/ip_unprivileged_port_start)" = "$unprivileged_port_start"

# A Tool needs neither user nor network namespaces. Disable their unprivileged creation where the
# kernel exposes the controls.
if [ -w /proc/sys/kernel/unprivileged_userns_clone ]; then
  printf '0\n' > /proc/sys/kernel/unprivileged_userns_clone
fi
if [ -w /proc/sys/user/max_user_namespaces ]; then
  printf '0\n' > /proc/sys/user/max_user_namespaces
fi

# Tool environments and the supervisor may hold session secrets. Never persist a process core;
# the live no-respawn canary deliberately aborts the supervisor after its receipt is flushed.
ulimit -c 0

# Engine file operations run in the supervisor and may create workspace parent directories. Keep
# those directories group-writable just like ordinary Tool output so every binding can collaborate
# through the shared workspace GID. Explicit Tool-created 0600 files remain binding-private.
umask 0002

exec /usr/local/lib/environment/supervisor-launcher /usr/local/bin/environment-guest
