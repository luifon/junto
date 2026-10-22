/* The system calls of V4L2 capture. A test gives a table of its own (v4l2_fake.c); every
 * function returns a negative errno value on failure. */
#ifndef JUNTO_V4L2_H
#define JUNTO_V4L2_H
#include <stddef.h>
#include <sys/types.h>

struct camera_system {
    void *context;
    int (*open)(void *context, const char *path);
    void (*close)(void *context, int fd);
    int (*ioctl)(void *context, int fd, unsigned long request, void *argument);
    /* The address of the mapping, or NULL with the error in *error. */
    void *(*map)(void *context, int fd, size_t length, off_t offset, int *error);
    void (*unmap)(void *context, void *data, size_t length);
    /* The returned events, 0 after the timeout in milliseconds. */
    int (*poll)(void *context, int fd, int timeout);
};
#endif
