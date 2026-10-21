/* A V4L2 capture device in memory, for the tests of v4l2.c without a kernel driver. The
 * product does not refer to this file, so the linker leaves it out of the product. */
#include <errno.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <pthread.h>
#include <stdint.h>
#include <stdlib.h>
#include <string.h>
#include <time.h>

#include "v4l2.h"

/* refuse: S_FMT answers with another mode, as a driver that adjusts the request; full_range:
 * S_FMT reports full-range quantization. */
struct fake_mode { uint32_t fourcc, width, height, numerator, denominator, refuse, full_range; };
/* One picture of the script. used UINT32_MAX: the size of the image. */
struct fake_frame { uint32_t flags, used, index_offset; uint8_t value; };
struct fake_device {
    const char *path;
    int open_error;
    uint32_t capabilities;
    const struct fake_mode *modes;
    size_t mode_count;
    uint32_t colorspace, encoding, quantization;
    int rate_error;
    uint32_t buffers;
    const struct fake_frame *frames;
    size_t frame_count;
};
/* What the code under test asked of one device. */
struct fake_record {
    uint32_t fourcc, width, height, numerator, denominator, buffers, mapped, opened, closed;
};

#define HANDLES 64
#define MAX_BUFFERS 64
struct handle {
    int used;
    size_t device;
    uint32_t sizeimage, count, streaming, queued[MAX_BUFFERS];
    void *memory[MAX_BUFFERS];
    size_t next;
};
struct fake_system {
    struct camera_system table;
    pthread_mutex_t lock;
    const struct fake_device *devices;
    size_t count;
    struct fake_record *records;
    /* The handle that holds the buffers of each device, as the queue of a V4L2 driver: -1
     * when none. Another handle gets EBUSY for S_FMT and REQBUFS. */
    int *owners;
    struct handle handles[HANDLES];
};

static struct fake_system *of(void *context) { return context; }

static int fake_open(void *context, const char *path) {
    struct fake_system *system = of(context);
    for (size_t d = 0; d < system->count; ++d) {
        if (strcmp(system->devices[d].path, path)) continue;
        if (system->devices[d].open_error) return -system->devices[d].open_error;
        pthread_mutex_lock(&system->lock);
        for (int h = 0; h < HANDLES; ++h) {
            if (system->handles[h].used) continue;
            system->handles[h] = (struct handle){.used = 1, .device = d};
            system->records[d].opened++;
            pthread_mutex_unlock(&system->lock);
            return 1000 + h;
        }
        pthread_mutex_unlock(&system->lock);
        return -EMFILE;
    }
    return -ENOENT;
}

static struct handle *handle(struct fake_system *system, int fd) {
    if (fd < 1000 || fd >= 1000 + HANDLES || !system->handles[fd - 1000].used) return NULL;
    return &system->handles[fd - 1000];
}

static void release(struct handle *h) {
    for (uint32_t i = 0; i < h->count; ++i) free(h->memory[i]);
    h->count = 0;
}

static void fake_close(void *context, int fd) {
    struct fake_system *system = of(context);
    pthread_mutex_lock(&system->lock);
    struct handle *h = handle(system, fd);
    if (h) {
        if (system->owners[h->device] == fd) system->owners[h->device] = -1;
        system->records[h->device].closed++;
        release(h);
        h->used = 0;
    }
    pthread_mutex_unlock(&system->lock);
}

static uint32_t line(uint32_t fourcc, uint32_t width) {
    return fourcc == V4L2_PIX_FMT_YUYV ? width * 2 : width;
}
static uint32_t image(uint32_t fourcc, uint32_t width, uint32_t height) {
    return fourcc == V4L2_PIX_FMT_YUYV ? width * height * 2 : width * height * 3 / 2;
}

/* The index-th distinct value among the modes, as chosen by key; -1 after the last. */
static long nth(const struct fake_device *device, uint32_t index, int (*same)(const struct fake_mode *, const struct fake_mode *),
                int (*matches)(const struct fake_mode *, const void *), const void *filter) {
    uint32_t seen = 0;
    for (size_t m = 0; m < device->mode_count; ++m) {
        const struct fake_mode *mode = &device->modes[m];
        if (filter && !matches(mode, filter)) continue;
        int earlier = 0;
        for (size_t e = 0; e < m && !earlier; ++e)
            if ((!filter || matches(&device->modes[e], filter)) && same(&device->modes[e], mode)) earlier = 1;
        if (earlier) continue;
        if (seen++ == index) return (long)m;
    }
    return -1;
}
static int same_fourcc(const struct fake_mode *a, const struct fake_mode *b) { return a->fourcc == b->fourcc; }
static int same_size(const struct fake_mode *a, const struct fake_mode *b) {
    return a->width == b->width && a->height == b->height;
}
static int same_rate(const struct fake_mode *a, const struct fake_mode *b) {
    return a->numerator == b->numerator && a->denominator == b->denominator;
}
static int of_fourcc(const struct fake_mode *mode, const void *filter) {
    return mode->fourcc == *(const uint32_t *)filter;
}
static int of_size(const struct fake_mode *mode, const void *filter) {
    const uint32_t *key = filter;
    return mode->fourcc == key[0] && mode->width == key[1] && mode->height == key[2] && mode->numerator;
}

static int control(struct fake_system *system, int fd, struct handle *h, unsigned long request, void *argument) {
    const struct fake_device *device = &system->devices[h->device];
    struct fake_record *record = &system->records[h->device];
    int *owner = &system->owners[h->device];
    if ((request == VIDIOC_S_FMT || request == VIDIOC_REQBUFS) && *owner != -1 && *owner != fd) return -EBUSY;
    switch (request) {
    case VIDIOC_QUERYCAP: {
        struct v4l2_capability *cap = argument;
        memset(cap, 0, sizeof(*cap));
        cap->capabilities = device->capabilities | V4L2_CAP_DEVICE_CAPS;
        cap->device_caps = device->capabilities;
        return 0;
    }
    case VIDIOC_ENUM_FMT: {
        struct v4l2_fmtdesc *description = argument;
        if (description->type != V4L2_BUF_TYPE_VIDEO_CAPTURE) return -EINVAL;
        long m = nth(device, description->index, same_fourcc, NULL, NULL);
        if (m < 0) return -EINVAL;
        description->pixelformat = device->modes[m].fourcc;
        description->flags = device->modes[m].fourcc == V4L2_PIX_FMT_MJPEG ? V4L2_FMT_FLAG_COMPRESSED : 0;
        return 0;
    }
    case VIDIOC_ENUM_FRAMESIZES: {
        struct v4l2_frmsizeenum *size = argument;
        long m = nth(device, size->index, same_size, of_fourcc, &size->pixel_format);
        if (m < 0) return -EINVAL;
        size->type = V4L2_FRMSIZE_TYPE_DISCRETE;
        size->discrete.width = device->modes[m].width;
        size->discrete.height = device->modes[m].height;
        return 0;
    }
    case VIDIOC_ENUM_FRAMEINTERVALS: {
        struct v4l2_frmivalenum *interval = argument;
        uint32_t key[3] = {interval->pixel_format, interval->width, interval->height};
        long m = nth(device, interval->index, same_rate, of_size, key);
        if (m < 0) return -EINVAL;
        interval->type = V4L2_FRMIVAL_TYPE_DISCRETE;
        interval->discrete.numerator = device->modes[m].numerator;
        interval->discrete.denominator = device->modes[m].denominator;
        return 0;
    }
    case VIDIOC_S_FMT: {
        struct v4l2_format *format = argument;
        if (format->type != V4L2_BUF_TYPE_VIDEO_CAPTURE || !device->mode_count) return -EINVAL;
        struct v4l2_pix_format *pix = &format->fmt.pix;
        const struct fake_mode *mode = NULL;
        for (size_t m = 0; m < device->mode_count; ++m)
            if (device->modes[m].fourcc == pix->pixelformat && device->modes[m].width == pix->width &&
                device->modes[m].height == pix->height && !device->modes[m].refuse) { mode = &device->modes[m]; break; }
        for (size_t m = 0; m < device->mode_count && !mode; ++m)
            if (!device->modes[m].refuse) mode = &device->modes[m];
        if (!mode) mode = &device->modes[0];
        pix->pixelformat = mode->fourcc;
        pix->width = mode->width;
        pix->height = mode->height;
        pix->field = V4L2_FIELD_NONE;
        pix->bytesperline = line(mode->fourcc, mode->width);
        pix->sizeimage = h->sizeimage = image(mode->fourcc, mode->width, mode->height);
        pix->colorspace = device->colorspace;
        pix->ycbcr_enc = device->encoding;
        pix->quantization = mode->full_range ? V4L2_QUANTIZATION_FULL_RANGE : device->quantization;
        record->fourcc = mode->fourcc;
        record->width = mode->width;
        record->height = mode->height;
        return 0;
    }
    case VIDIOC_G_PARM: {
        struct v4l2_streamparm *parameters = argument;
        memset(&parameters->parm, 0, sizeof(parameters->parm));
        parameters->parm.capture.capability = V4L2_CAP_TIMEPERFRAME;
        return 0;
    }
    case VIDIOC_S_PARM: {
        struct v4l2_streamparm *parameters = argument;
        if (device->rate_error) return -device->rate_error;
        record->numerator = parameters->parm.capture.timeperframe.numerator;
        record->denominator = parameters->parm.capture.timeperframe.denominator;
        return 0;
    }
    case VIDIOC_REQBUFS: {
        struct v4l2_requestbuffers *request = argument;
        release(h);
        if (!request->count) { *owner = -1; return 0; }
        uint32_t count = device->buffers ? device->buffers : request->count;
        if (count > MAX_BUFFERS) count = MAX_BUFFERS;
        for (uint32_t i = 0; i < count; ++i) {
            h->memory[i] = calloc(1, h->sizeimage);
            h->queued[i] = 0;
            if (!h->memory[i]) { h->count = i; release(h); return -ENOMEM; }
        }
        h->count = request->count = count;
        record->buffers = count;
        *owner = fd;
        return 0;
    }
    case VIDIOC_QUERYBUF: {
        struct v4l2_buffer *buffer = argument;
        if (buffer->index >= h->count) return -EINVAL;
        buffer->length = h->sizeimage;
        buffer->m.offset = buffer->index * 4096;
        return 0;
    }
    case VIDIOC_QBUF: {
        struct v4l2_buffer *buffer = argument;
        if (buffer->index >= h->count || h->queued[buffer->index]) return -EINVAL;
        h->queued[buffer->index] = 1;
        return 0;
    }
    case VIDIOC_STREAMON: h->streaming = 1; return 0;
    case VIDIOC_STREAMOFF: h->streaming = 0; return 0;
    case VIDIOC_DQBUF: {
        struct v4l2_buffer *buffer = argument;
        if (!h->streaming || h->next >= device->frame_count) return -EAGAIN;
        for (uint32_t i = 0; i < h->count; ++i) {
            if (!h->queued[i]) continue;
            const struct fake_frame *frame = &device->frames[h->next++];
            uint32_t used = frame->used == UINT32_MAX ? h->sizeimage : frame->used;
            memset(h->memory[i], frame->value, used < h->sizeimage ? used : h->sizeimage);
            h->queued[i] = 0;
            buffer->index = i + frame->index_offset;
            buffer->bytesused = used;
            buffer->flags = frame->flags | V4L2_BUF_FLAG_TIMESTAMP_MONOTONIC;
            buffer->timestamp.tv_sec = (long)(h->next / 30);
            buffer->timestamp.tv_usec = (long)(h->next % 30) * 33333;
            /* A dequeued buffer with a wrong index is lost to the driver as well. */
            return 0;
        }
        return -EAGAIN;
    }
    default:
        return -ENOTTY;
    }
}

static int fake_ioctl(void *context, int fd, unsigned long request, void *argument) {
    struct fake_system *system = of(context);
    pthread_mutex_lock(&system->lock);
    struct handle *h = handle(system, fd);
    int result = h ? control(system, fd, h, request, argument) : -EBADF;
    pthread_mutex_unlock(&system->lock);
    return result;
}

static void *fake_map(void *context, int fd, size_t length, off_t offset, int *error) {
    struct fake_system *system = of(context);
    pthread_mutex_lock(&system->lock);
    struct handle *h = handle(system, fd);
    void *data = NULL;
    size_t index = (size_t)offset / 4096;
    if (h && index < h->count && length == h->sizeimage) {
        data = h->memory[index];
        system->records[h->device].mapped++;
    }
    pthread_mutex_unlock(&system->lock);
    if (!data) *error = -EINVAL;
    return data;
}

static void fake_unmap(void *context, void *data, size_t length) { (void)context; (void)data; (void)length; }

static int fake_poll(void *context, int fd, int timeout) {
    struct fake_system *system = of(context);
    pthread_mutex_lock(&system->lock);
    struct handle *h = handle(system, fd);
    int ready = 0;
    if (h && h->streaming && h->next < system->devices[h->device].frame_count)
        for (uint32_t i = 0; i < h->count; ++i) ready |= (int)h->queued[i];
    pthread_mutex_unlock(&system->lock);
    if (!h) return POLLNVAL;
    if (ready) return POLLIN;
    struct timespec wait = {timeout / 1000, (long)(timeout % 1000) * 1000000L};
    nanosleep(&wait, NULL);
    return 0;
}

struct fake_system *junto_fake_system(const struct fake_device *devices, size_t count) {
    struct fake_system *system = calloc(1, sizeof(*system));
    if (!system) return NULL;
    system->records = calloc(count ? count : 1, sizeof(*system->records));
    system->owners = malloc((count ? count : 1) * sizeof(*system->owners));
    if (!system->records || !system->owners) { free(system->records); free(system->owners); free(system); return NULL; }
    for (size_t d = 0; d < count; ++d) system->owners[d] = -1;
    pthread_mutex_init(&system->lock, NULL);
    system->devices = devices;
    system->count = count;
    system->table = (struct camera_system){system, fake_open, fake_close, fake_ioctl, fake_map, fake_unmap, fake_poll};
    return system;
}

void junto_fake_record(struct fake_system *system, size_t device, struct fake_record *out) {
    pthread_mutex_lock(&system->lock);
    *out = system->records[device];
    pthread_mutex_unlock(&system->lock);
}
