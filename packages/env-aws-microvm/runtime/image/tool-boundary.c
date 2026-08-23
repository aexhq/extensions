#include <sys/prctl.h>
#include <unistd.h>

/*
 * Runs inside the final dynamically linked Node/bash image, after exec has reset dumpability.
 * The supervisor supplies this root-owned library after all customer environment assignments.
 * An innocent secret-bearing Tool therefore cannot be inspected through sensitive procfs files
 * by an unrelated process that deliberately shares its unprivileged UID.
 */
__attribute__((constructor)) static void environment_tool_boundary(void) {
    if (prctl(PR_SET_DUMPABLE, 0, 0, 0, 0) == -1) {
        _exit(125);
    }
}
