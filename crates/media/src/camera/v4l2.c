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

struct camera_format {
    uint32_t width, height, stride, pixel, matrix;
};
struct mapping { void *data; size_t length; };
struct camera {
    int fd, streaming;
    struct mapping buffers[4];
};

static int call(int fd, unsigned long op, void *value) {
    int result;
    do { result = ioctl(fd, op, value); } while (result < 0 && errno == EINTR);
    return result;
}

int junto_camera_probe(const char *path) {
    int fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd < 0) return -errno;
    struct v4l2_capability cap = {0};
    int result = call(fd, VIDIOC_QUERYCAP, &cap);
    int error = errno;
    close(fd);
    if (result < 0) return -error;
    uint32_t flags = cap.capabilities & V4L2_CAP_DEVICE_CAPS ? cap.device_caps : cap.capabilities;
    return (flags & V4L2_CAP_VIDEO_CAPTURE) && (flags & V4L2_CAP_STREAMING) ? 1 : 0;
}

void junto_camera_close(struct camera *camera) {
    if (!camera) return;
    if (camera->streaming) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        call(camera->fd, VIDIOC_STREAMOFF, &type);
    }
    for (size_t i = 0; i < 4; ++i)
        if (camera->buffers[i].length) munmap(camera->buffers[i].data, camera->buffers[i].length);
    close(camera->fd);
    free(camera);
}

static int format(int fd, struct camera_format *out) {
    const uint32_t sizes[][2] = {{1280,720},{640,480},{352,288}};
    const uint32_t pixels[] = {V4L2_PIX_FMT_NV12,V4L2_PIX_FMT_YUV420,V4L2_PIX_FMT_YUYV};
    for (size_t s = 0; s < 3; ++s) for (size_t p = 0; p < 3; ++p) {
        struct v4l2_format f = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
        f.fmt.pix.width = sizes[s][0];
        f.fmt.pix.height = sizes[s][1];
        f.fmt.pix.pixelformat = pixels[p];
        f.fmt.pix.field = V4L2_FIELD_NONE;
        f.fmt.pix.priv = V4L2_PIX_FMT_PRIV_MAGIC;
        if (call(fd, VIDIOC_S_FMT, &f) < 0) {
            if (errno == EINVAL) continue;
            return -errno;
        }
        const struct v4l2_pix_format *pix = &f.fmt.pix;
        if (pix->width != sizes[s][0] || pix->height != sizes[s][1] ||
            pix->pixelformat != pixels[p] || pix->field != V4L2_FIELD_NONE) continue;
        uint32_t range = pix->quantization;
        if (range == V4L2_QUANTIZATION_DEFAULT)
            range = V4L2_MAP_QUANTIZATION_DEFAULT(0, pix->colorspace, pix->ycbcr_enc);
        if (range != V4L2_QUANTIZATION_LIM_RANGE) continue;
        uint32_t matrix = pix->ycbcr_enc;
        if (matrix == V4L2_YCBCR_ENC_DEFAULT && pix->colorspace != V4L2_COLORSPACE_DEFAULT)
            matrix = V4L2_MAP_YCBCR_ENC_DEFAULT(pix->colorspace);
        *out = (struct camera_format){pix->width,pix->height,pix->bytesperline,(uint32_t)p,matrix};
        struct v4l2_streamparm rate = {.type = V4L2_BUF_TYPE_VIDEO_CAPTURE};
        if (call(fd, VIDIOC_G_PARM, &rate) == 0 &&
            (rate.parm.capture.capability & V4L2_CAP_TIMEPERFRAME)) {
            rate.parm.capture.timeperframe = (struct v4l2_fract){1,30};
            if (call(fd, VIDIOC_S_PARM, &rate) < 0) return -errno;
        }
        return 0;
    }
    return -EOPNOTSUPP;
}

int junto_camera_open(const char *path, struct camera_format *out, struct camera **result) {
    struct camera *camera = calloc(1, sizeof(*camera));
    if (!camera) return -ENOMEM;
    camera->fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (camera->fd < 0) { int error = errno; free(camera); return -error; }
    int error = format(camera->fd, out);
    if (error) goto fail;
    struct v4l2_requestbuffers request = {.count=4,.type=V4L2_BUF_TYPE_VIDEO_CAPTURE,.memory=V4L2_MEMORY_MMAP};
    if (call(camera->fd, VIDIOC_REQBUFS, &request) < 0) { error = -errno; goto fail; }
    if (!request.count || request.count > 4) { error = -EOPNOTSUPP; goto fail; }
    for (uint32_t i = 0; i < request.count; ++i) {
        struct v4l2_buffer buffer = {.index=i,.type=V4L2_BUF_TYPE_VIDEO_CAPTURE,.memory=V4L2_MEMORY_MMAP};
        if (call(camera->fd, VIDIOC_QUERYBUF, &buffer) < 0) { error = -errno; goto fail; }
        if (!buffer.length || buffer.length > 32 * 1024 * 1024) { error = -EOPNOTSUPP; goto fail; }
        void *data = mmap(NULL, buffer.length, PROT_READ | PROT_WRITE, MAP_SHARED, camera->fd, buffer.m.offset);
        if (data == MAP_FAILED) { error = -errno; goto fail; }
        camera->buffers[i] = (struct mapping){data,buffer.length};
        if (call(camera->fd, VIDIOC_QBUF, &buffer) < 0) { error = -errno; goto fail; }
    }
    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (call(camera->fd, VIDIOC_STREAMON, &type) < 0) { error = -errno; goto fail; }
    camera->streaming = 1;
    *result = camera;
    return 0;
fail:
    junto_camera_close(camera);
    return error;
}

int junto_camera_next(struct camera *camera, uint8_t *output, size_t length) {
    struct pollfd descriptor = {.fd=camera->fd,.events=POLLIN};
    int result = poll(&descriptor, 1, 100);
    if (result < 0) return errno == EINTR ? 0 : -errno;
    if (!result) return 0;
    if (descriptor.revents & (POLLHUP | POLLNVAL)) return -ENODEV;
    struct v4l2_buffer buffer = {.type=V4L2_BUF_TYPE_VIDEO_CAPTURE,.memory=V4L2_MEMORY_MMAP};
    if (call(camera->fd, VIDIOC_DQBUF, &buffer) < 0) return errno == EAGAIN ? 0 : -errno;
    int error = 1;
    if (buffer.index >= 4 || buffer.bytesused > camera->buffers[buffer.index].length || buffer.bytesused < length)
        error = -EOVERFLOW;
    else if (buffer.flags & V4L2_BUF_FLAG_ERROR) error = 0;
    else memcpy(output, camera->buffers[buffer.index].data, length);
    if (call(camera->fd, VIDIOC_QBUF, &buffer) < 0) return -errno;
    return error;
}
