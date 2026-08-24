#include <errno.h>
#include <fcntl.h>
#include <stdio.h>
#include <string.h>
#include <unistd.h>

// Image-conformance helper only. It deliberately inherits the caller's environment and then
// remains alive after a static exec so the sibling-binding fixture can attack procfs. Static exec
// resets PR_SET_DUMPABLE and ignores LD_PRELOAD, making this the important adversarial case.
int main(int argc, char **argv) {
    if (argc != 2) {
        return 64;
    }
    int fd = open(argv[1], O_WRONLY | O_CREAT | O_TRUNC | O_CLOEXEC, 0660);
    if (fd == -1) {
        return 65;
    }
    char pid[32];
    int length = snprintf(pid, sizeof(pid), "%ld", (long)getpid());
    if (length <= 0 || length >= (int)sizeof(pid)
        || write(fd, pid, (size_t)length) != length || close(fd) == -1) {
        return 66;
    }
    for (;;) {
        pause();
        if (errno != EINTR) {
            return 67;
        }
    }
}
