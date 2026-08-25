#!/bin/sh
# Root boot boundary for the trusted Environment supervisor and every Tool uid.
set -eu

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
