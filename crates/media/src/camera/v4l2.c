/* The V4L2 calls of camera capture. The decisions are in v4l2.rs and linux.rs. */
#include <errno.h>
#include <fcntl.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

#include "v4l2.h"

/* The most buffers that a driver may give for the four that are asked for. */
#define MAX_BUFFERS 32

struct camera_format {
    uint32_t width, height, stride, fourcc, field, encoding, quantization;
    int32_t rate_error;
};
struct camera_request { uint32_t fourcc, width, height, numerator, denominator; };
struct camera_range { uint32_t kind, values[6]; };
struct camera_buffer { uint32_t index, used, flags, count, length; uint64_t timestamp; };

struct mapping { void *data; size_t length; };
struct camera {
    const struct camera_system *system;
    int fd, streaming;
    uint32_t count;
    struct mapping buffers[MAX_BUFFERS];
};

static int native_open(void *context, const char *path) {
    (void)context;
    int fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    return fd < 0 ? -errno : fd;
}
static void native_close(void *context, int fd) { (void)context; close(fd); }
static int native_ioctl(void *context, int fd, unsigned long request, void *argument) {
    (void)context;
    int result;
    do { result = ioctl(fd, request, argument); } while (result < 0 && errno == EINTR);
    return result < 0 ? -errno : result;
}
static void *native_map(void *context, int fd, size_t length, off_t offset, int *error) {
    (void)context;
    void *data = mmap(NULL, length, PROT_READ | PROT_WRITE, MAP_SHARED, fd, offset);
    if (data != MAP_FAILED) return data;
    *error = -errno;
    return NULL;
}
static void native_unmap(void *context, void *data, size_t length) { (void)context; munmap(data, length); }
static int native_poll(void *context, int fd, int timeout) {
    (void)context;
    struct pollfd descriptor = {.fd = fd, .events = POLLIN};
    int result = poll(&descriptor, 1, timeout);
    if (result < 0) return errno == EINTR ? 0 : -errno;
    return result ? descriptor.revents : 0;
}
static const struct camera_system native = {
    NULL, native_open, native_close, native_ioctl, native_map, native_unmap, native_poll,
};

static int call(struct camera *camera, unsigned long request, void *argument) {
    return camera->system->ioctl(camera->system->context, camera->fd, request, argument);
}

void junto_camera_close(struct camera *camera) {
    if (!camera) return;
    const struct camera_system *system = camera->system;
    if (camera->streaming) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        call(camera, VIDIOC_STREAMOFF, &type);
    }
    for (uint32_t i = 0; i < camera->count; ++i)
        if (camera->buffers[i].data) system->unmap(system->context, camera->buffers[i].data, camera->buffers[i].length);
    system->close(system->context, camera->fd);
    free(camera);
}

/* 0 with a camera; -EOPNOTSUPP for a node that is not a single-planar streaming capture
 * device; another negative errno value when the node cannot be opened. */
int junto_camera_open(const struct camera_system *system, const char *path, struct camera **result) {
    struct camera *camera = calloc(1, sizeof(*camera));
    if (!camera) return -ENOMEM;
    camera->system = system ? system : &native;
    camera->fd = camera->system->open(camera->system->context, path);
    if (camera->fd < 0) { int error = camera->fd; free(camera); return error; }
    struct v4l2_capability cap = {0};
    int error = call(camera, VIDIOC_QUERYCAP, &cap);
    uint32_t flags = cap.capabilities & V4L2_CAP_DEVICE_CAPS ? cap.device_caps : cap.capabilities;
    if (!error && !((flags & V4L2_CAP_VIDEO_CAPTURE) && (flags & V4L2_CAP_STREAMING))) error = -EOPNOTSUPP;
    if (error) { junto_camera_close(camera); return error; }
    *result = camera;
    return 0;
}

/* The enumerations return 1 for an entry, 0 after the last, and a negative errno value. A
 * driver without the enumeration has no entries. */
static int entry(int result) { return result == -EINVAL || result == -ENOTTY ? 0 : result < 0 ? result : 1; }

int junto_camera_format(struct camera *camera, uint32_t index, uint32_t *fourcc) {
    struct v4l2_fmtdesc description = {.index = index, .type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
    int result = entry(call(camera, VIDIOC_ENUM_FMT, &description));
    if (result == 1) *fourcc = description.pixelformat;
    return result;
}

int junto_camera_size(struct camera *camera, uint32_t fourcc, uint32_t index, struct camera_range *out) {
    struct v4l2_frmsizeenum size = {.index = index, .pixel_format = fourcc};
    int result = entry(call(camera, VIDIOC_ENUM_FRAMESIZES, &size));
    if (result != 1) return result;
    out->kind = size.type;
    if (size.type == V4L2_FRMSIZE_TYPE_DISCRETE) {
        uint32_t values[6] = {size.discrete.width, size.discrete.height};
        memcpy(out->values, values, sizeof(values));
    } else {
        const struct v4l2_frmsize_stepwise *s = &size.stepwise;
        uint32_t values[6] = {s->min_width, s->max_width, s->step_width, s->min_height, s->max_height, s->step_height};
        memcpy(out->values, values, sizeof(values));
    }
    return 1;
}

int junto_camera_interval(struct camera *camera, uint32_t fourcc, uint32_t width, uint32_t height,
                          uint32_t index, struct camera_range *out) {
    struct v4l2_frmivalenum interval = {.index = index, .pixel_format = fourcc, .width = width, .height = height};
    int result = entry(call(camera, VIDIOC_ENUM_FRAMEINTERVALS, &interval));
    if (result != 1) return result;
    out->kind = interval.type;
    if (interval.type == V4L2_FRMIVAL_TYPE_DISCRETE) {
        uint32_t values[6] = {interval.discrete.numerator, interval.discrete.denominator};
        memcpy(out->values, values, sizeof(values));
    } else {
        const struct v4l2_frmival_stepwise *s = &interval.stepwise;
        uint32_t values[6] = {s->min.numerator, s->min.denominator, s->max.numerator, s->max.denominator,
                              s->step.numerator, s->step.denominator};
        memcpy(out->values, values, sizeof(values));
    }
    return 1;
}

/* Sets the format and the frame interval, maps the buffers and starts streaming: 0 when it
 * streams, 1 when the driver set another size or pixel format, a negative errno value. The
 * format that the driver set is in *out, with the defaults of its colorspace resolved; the
 * caller checks it. A refused interval is reported in out->rate_error for the caller. */
int junto_camera_start(struct camera *camera, const struct camera_request *request, struct camera_format *out) {
    struct v4l2_format f = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
    f.fmt.pix.width = request->width;
    f.fmt.pix.height = request->height;
    f.fmt.pix.pixelformat = request->fourcc;
    f.fmt.pix.field = V4L2_FIELD_NONE;
    f.fmt.pix.priv = V4L2_PIX_FMT_PRIV_MAGIC;
    int error = call(camera, VIDIOC_S_FMT, &f);
    if (error) return error;
    const struct v4l2_pix_format *pix = &f.fmt.pix;
    uint32_t encoding = pix->ycbcr_enc;
    if (encoding == V4L2_YCBCR_ENC_DEFAULT && pix->colorspace != V4L2_COLORSPACE_DEFAULT)
        encoding = V4L2_MAP_YCBCR_ENC_DEFAULT(pix->colorspace);
    uint32_t quantization = pix->quantization;
    if (quantization == V4L2_QUANTIZATION_DEFAULT)
        quantization = V4L2_MAP_QUANTIZATION_DEFAULT(0, pix->colorspace, pix->ycbcr_enc);
    *out = (struct camera_format){pix->width, pix->height, pix->bytesperline, pix->pixelformat,
                                  pix->field, encoding, quantization, 0};
    if (pix->width != request->width || pix->height != request->height || pix->pixelformat != request->fourcc)
        return 1;
    struct v4l2_streamparm rate = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
    if (call(camera, VIDIOC_G_PARM, &rate) == 0 && (rate.parm.capture.capability & V4L2_CAP_TIMEPERFRAME)) {
        rate.parm.capture.timeperframe = (struct v4l2_fract){request->numerator, request->denominator};
        out->rate_error = call(camera, VIDIOC_S_PARM, &rate);
    }
    struct v4l2_requestbuffers buffers = {.count = 4, .type = V4L2_BUF_TYPE_VIDEO_CAPTURE, .memory = V4L2_MEMORY_MMAP};
    if ((error = call(camera, VIDIOC_REQBUFS, &buffers))) return error;
    if (!buffers.count || buffers.count > MAX_BUFFERS) return -EOPNOTSUPP;
    camera->count = buffers.count;
    for (uint32_t i = 0; i < buffers.count; ++i) {
        struct v4l2_buffer buffer = {.index = i, .type = V4L2_BUF_TYPE_VIDEO_CAPTURE, .memory = V4L2_MEMORY_MMAP};
        if ((error = call(camera, VIDIOC_QUERYBUF, &buffer))) return error;
        if (!buffer.length || buffer.length > 32 * 1024 * 1024) return -EOPNOTSUPP;
        void *data = camera->system->map(camera->system->context, camera->fd, buffer.length, buffer.m.offset, &error);
        if (!data) return error;
        camera->buffers[i] = (struct mapping){data, buffer.length};
        if ((error = call(camera, VIDIOC_QBUF, &buffer))) return error;
    }
    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if ((error = call(camera, VIDIOC_STREAMON, &type))) return error;
    camera->streaming = 1;
    return 0;
}

/* 1 with a dequeued buffer in *out, which the caller gives back with junto_camera_release;
 * 0 when no buffer came within the timeout; a negative errno value. */
int junto_camera_next(struct camera *camera, int timeout, struct camera_buffer *out) {
    int events = camera->system->poll(camera->system->context, camera->fd, timeout);
    if (events < 0) return events;
    if (!events) return 0;
    if (events & (POLLHUP | POLLNVAL)) return -ENODEV;
    if (events & POLLERR) return -EIO;
    struct v4l2_buffer buffer = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE, .memory = V4L2_MEMORY_MMAP};
    int error = call(camera, VIDIOC_DQBUF, &buffer);
    if (error) return error == -EAGAIN ? 0 : error;
    uint64_t timestamp = 0;
    if ((buffer.flags & V4L2_BUF_FLAG_TIMESTAMP_MASK) == V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC)
        timestamp = (uint64_t)buffer.timestamp.tv_sec * 1000000000u + (uint64_t)buffer.timestamp.tv_usec * 1000u;
    uint32_t length = buffer.index < camera->count ? (uint32_t)camera->buffers[buffer.index].length : 0;
    *out = (struct camera_buffer){buffer.index, buffer.bytesused, buffer.flags, camera->count, length, timestamp};
    return 1;
}

/* Copies the first length bytes of a dequeued buffer to output, when output is not NULL,
 * and queues the buffer again. */
int junto_camera_release(struct camera *camera, uint32_t index, uint8_t *output, size_t length) {
    if (index >= camera->count || length > camera->buffers[index].length) return -EINVAL;
    if (output) memcpy(output, camera->buffers[index].data, length);
    struct v4l2_buffer buffer = {.index = index, .type = V4L2_BUF_TYPE_VIDEO_CAPTURE, .memory = V4L2_MEMORY_MMAP};
    return call(camera, VIDIOC_QBUF, &buffer);
}
