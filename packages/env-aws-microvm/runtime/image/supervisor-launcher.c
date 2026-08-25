#define _GNU_SOURCE

#include <errno.h>
#include <grp.h>
#include <linux/capability.h>
#include <pwd.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/prctl.h>
#include <sys/syscall.h>
#include <sys/types.h>
#include <unistd.h>

enum {
    SUPERVISOR_UID = 1001,
    SUPERVISOR_GID = 1001,
};

static void fail(const char *operation) {
    fprintf(stderr, "aex-supervisor-launcher: %s: %s\n", operation, strerror(errno));
    exit(111);
}

static void require(int condition, const char *message) {
    if (!condition) {
        fprintf(stderr, "aex-supervisor-launcher: %s\n", message);
        exit(111);
    }
}

static void set_supervisor_capabilities(void) {
    _Static_assert(CAP_KILL < 32 && CAP_SETGID < 32 && CAP_SETUID < 32,
                   "supervisor capabilities must fit in the first capability word");
    const uint32_t supervisor_mask =
        (1U << CAP_KILL) | (1U << CAP_SETGID) | (1U << CAP_SETUID);
    struct __user_cap_header_struct header = {
        .version = _LINUX_CAPABILITY_VERSION_3,
        .pid = 0,
    };
    struct __user_cap_data_struct data[2] = {{0}};
    data[0].effective = supervisor_mask;
    data[0].permitted = supervisor_mask;
    data[0].inheritable = supervisor_mask;
    if (syscall(SYS_capset, &header, data) < 0) {
        fail("capset");
    }
}

static void raise_supervisor_ambient_capabilities(void) {
    const int supervisor_capabilities[] = {CAP_KILL, CAP_SETGID, CAP_SETUID};
    if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL, 0, 0, 0) < 0) {
        fail("prctl(PR_CAP_AMBIENT_CLEAR_ALL)");
    }
    for (size_t index = 0;
         index < sizeof(supervisor_capabilities) / sizeof(supervisor_capabilities[0]);
         ++index) {
        if (prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_RAISE,
                  supervisor_capabilities[index], 0, 0) < 0) {
            fail("prctl(PR_CAP_AMBIENT_RAISE)");
        }
    }
}

int main(int argc, char **argv) {
    require(argc == 2, "expected the Environment supervisor executable");

    struct passwd *supervisor = getpwnam("environment");
    require(supervisor != NULL, "Environment supervisor account is unavailable");
    require(supervisor->pw_uid == SUPERVISOR_UID && supervisor->pw_gid == SUPERVISOR_GID,
            "Environment supervisor identity does not match the sealed image");
    if (initgroups("environment", supervisor->pw_gid) < 0) {
        fail("initgroups");
    }
    if (setgid(supervisor->pw_gid) < 0) {
        fail("setgid");
    }
    /*
     * Provider filesystems may ignore executable file capabilities. Seed the exact inheritable
     * set while privileged, preserve only that permitted set across the uid transition, restore
     * it as effective, and carry it through exec as ambient capabilities. The supervisor clears
     * every capability set in each untrusted child before executing Tool code.
     */
    if (prctl(PR_SET_KEEPCAPS, 1, 0, 0, 0) < 0) {
        fail("prctl(PR_SET_KEEPCAPS)");
    }
    set_supervisor_capabilities();
    if (setuid(supervisor->pw_uid) < 0) {
        fail("setuid");
    }
    set_supervisor_capabilities();
    if (prctl(PR_SET_KEEPCAPS, 0, 0, 0, 0) < 0) {
        fail("prctl(PR_SET_KEEPCAPS clear)");
    }
    raise_supervisor_ambient_capabilities();
    require(geteuid() == SUPERVISOR_UID && getegid() == SUPERVISOR_GID,
            "failed to enter the Environment supervisor identity");

    if (setenv("HOME", "/home/agent", 1) < 0 || setenv("USER", "environment", 1) < 0 ||
        setenv("LOGNAME", "environment", 1) < 0) {
        fail("setenv");
    }
    char *const child_argv[] = {argv[1], NULL};
    execv(argv[1], child_argv);
    fail("execv");
}
