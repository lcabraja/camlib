// V4L2 capture following OBS's plugins/linux-v4l2 flow: enumerate /sys/class/video4linux,
// keep capture-capable nodes, set format and frame interval, mmap buffers, then poll and dequeue.
// Pixel conversion happens in Rust; this file only owns the kernel interface.

#define _GNU_SOURCE
#include <dirent.h>
#include <errno.h>
#include <fcntl.h>
#include <limits.h>
#include <linux/videodev2.h>
#include <poll.h>
#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ioctl.h>
#include <sys/mman.h>
#include <unistd.h>

typedef struct {
    uint32_t fourcc;
    uint32_t width;
    uint32_t height;
    // Frame interval: fps = denominator / numerator.
    uint32_t numerator;
    uint32_t denominator;
} camlib_v4l2_mode;

typedef void (*camlib_v4l2_frame_cb)(void *ctx, const uint8_t *data, size_t length);

#define CAMLIB_BUFFERS 4

typedef struct {
    int fd;
    uint32_t count;
    void *start[CAMLIB_BUFFERS];
    size_t length[CAMLIB_BUFFERS];
    int streaming;
} camlib_v4l2;

static int xioctl(int fd, unsigned long request, void *arg)
{
    int r;
    do {
        r = ioctl(fd, request, arg);
    } while (r == -1 && errno == EINTR);
    return r;
}

static void set_error(char *error, size_t error_len, const char *what)
{
    if (error && error_len > 0) {
        snprintf(error, error_len, "%s: %s", what, strerror(errno));
    }
}

static uint32_t capture_caps(int fd, struct v4l2_capability *cap)
{
    memset(cap, 0, sizeof(*cap));
    if (xioctl(fd, VIDIOC_QUERYCAP, cap) == -1) {
        return 0;
    }
    return (cap->capabilities & V4L2_CAP_DEVICE_CAPS) ? cap->device_caps : cap->capabilities;
}

static void clean(char *text)
{
    for (; *text; ++text) {
        if (*text == '\t' || *text == '\n') {
            *text = ' ';
        }
    }
}

static int compare_names(const void *a, const void *b)
{
    const char *left = *(const char *const *)a;
    const char *right = *(const char *const *)b;
    // Order video2 before video10.
    size_t left_len = strlen(left), right_len = strlen(right);
    if (left_len != right_len) {
        return left_len < right_len ? -1 : 1;
    }
    return strcmp(left, right);
}

// Prefer the persistent /dev/v4l/by-id link for a node so ids survive re-enumeration.
static void stable_path(const char *node, char *out, size_t out_len)
{
    snprintf(out, out_len, "%s", node);
    DIR *dir = opendir("/dev/v4l/by-id");
    if (!dir) {
        return;
    }
    struct dirent *entry;
    char link[PATH_MAX], target[PATH_MAX];
    while ((entry = readdir(dir)) != NULL) {
        if (entry->d_name[0] == '.') {
            continue;
        }
        snprintf(link, sizeof link, "/dev/v4l/by-id/%s", entry->d_name);
        if (realpath(link, target) && strcmp(target, node) == 0) {
            snprintf(out, out_len, "%s", link);
            break;
        }
    }
    closedir(dir);
}

// Reads /sys/class/video4linux/<node>/<file> without its trailing newline.
static int sysfs_value(const char *node, const char *file, char *out, size_t out_len)
{
    char path[PATH_MAX];
    snprintf(path, sizeof path, "/sys/class/video4linux/%s/%s", node, file);
    FILE *f = fopen(path, "r");
    if (!f) {
        return -1;
    }
    int ok = fgets(out, (int)out_len, f) != NULL;
    fclose(f);
    if (!ok) {
        return -1;
    }
    out[strcspn(out, "\n")] = '\0';
    return 0;
}

// Lines of "id\tname\tbus_info\n". Returns the bytes needed including the terminator.
size_t camlib_v4l2_list(char *buffer, size_t buffer_len)
{
    DIR *dir = opendir("/sys/class/video4linux");
    char *names[256];
    size_t name_count = 0;
    if (dir) {
        struct dirent *entry;
        while ((entry = readdir(dir)) != NULL && name_count < 256) {
            if (strncmp(entry->d_name, "video", 5) == 0) {
                names[name_count++] = strdup(entry->d_name);
            }
        }
        closedir(dir);
    }
    qsort(names, name_count, sizeof names[0], compare_names);

    size_t used = 0;
    char line[PATH_MAX + 128], node[PATH_MAX], id[PATH_MAX];
    for (size_t i = 0; i < name_count; ++i) {
        snprintf(node, sizeof node, "/dev/%s", names[i]);
        char card[64] = "", bus[64] = "";
        int fd = open(node, O_RDWR | O_NONBLOCK | O_CLOEXEC);
        if (fd == -1) {
            // Still list cameras the user may not open (usually not in the "video" group), so
            // opening reports why instead of the camera silently missing. Without capabilities,
            // sysfs index 0 marks a device's primary node rather than its metadata node.
            if (errno != EACCES || sysfs_value(names[i], "index", line, sizeof line) != 0 || strcmp(line, "0") != 0 ||
                sysfs_value(names[i], "name", card, sizeof card) != 0) {
                free(names[i]);
                continue;
            }
            snprintf(bus, sizeof bus, "permission denied");
        } else {
            struct v4l2_capability cap;
            uint32_t caps = capture_caps(fd, &cap);
            close(fd);
            // Metadata and output nodes of the same camera are skipped.
            if (!(caps & V4L2_CAP_VIDEO_CAPTURE) || !(caps & V4L2_CAP_STREAMING)) {
                free(names[i]);
                continue;
            }
            snprintf(card, sizeof card, "%s", (const char *)cap.card);
            snprintf(bus, sizeof bus, "%s", (const char *)cap.bus_info);
        }
        free(names[i]);
        stable_path(node, id, sizeof id);
        clean(card);
        clean(bus);
        clean(id);
        int n = snprintf(line, sizeof line, "%s\t%s\t%s\n", id, card, bus);
        if (n <= 0) {
            continue;
        }
        if (buffer && used + (size_t)n < buffer_len) {
            memcpy(buffer + used, line, (size_t)n);
        }
        used += (size_t)n;
    }
    if (buffer && buffer_len > 0) {
        buffer[used < buffer_len ? used : buffer_len - 1] = '\0';
    }
    return used + 1;
}

static void push_mode(camlib_v4l2_mode *modes, int capacity, int *count, uint32_t fourcc, uint32_t width,
                      uint32_t height, uint32_t numerator, uint32_t denominator)
{
    if (numerator == 0 || denominator == 0) {
        return;
    }
    if (*count < capacity) {
        modes[*count] = (camlib_v4l2_mode){fourcc, width, height, numerator, denominator};
    }
    ++*count;
}

static void enumerate_intervals(int fd, uint32_t fourcc, uint32_t width, uint32_t height, camlib_v4l2_mode *modes,
                                int capacity, int *count)
{
    struct v4l2_frmivalenum ival;
    memset(&ival, 0, sizeof ival);
    ival.pixel_format = fourcc;
    ival.width = width;
    ival.height = height;
    if (xioctl(fd, VIDIOC_ENUM_FRAMEINTERVALS, &ival) == -1) {
        // Drivers without interval enumeration still stream at some rate; assume 30fps.
        push_mode(modes, capacity, count, fourcc, width, height, 1, 30);
        return;
    }
    if (ival.type == V4L2_FRMIVAL_TYPE_DISCRETE) {
        do {
            push_mode(modes, capacity, count, fourcc, width, height, ival.discrete.numerator,
                      ival.discrete.denominator);
            ++ival.index;
        } while (xioctl(fd, VIDIOC_ENUM_FRAMEINTERVALS, &ival) == 0);
        return;
    }
    // Stepwise or continuous: offer the fastest and slowest intervals, and 30fps when inside.
    struct v4l2_fract fast = ival.stepwise.min, slow = ival.stepwise.max;
    push_mode(modes, capacity, count, fourcc, width, height, fast.numerator, fast.denominator);
    push_mode(modes, capacity, count, fourcc, width, height, slow.numerator, slow.denominator);
    if ((uint64_t)fast.numerator * 30 <= fast.denominator && (uint64_t)slow.numerator * 30 >= slow.denominator) {
        push_mode(modes, capacity, count, fourcc, width, height, 1, 30);
    }
}

// Writes up to `capacity` modes and returns how many exist, or -1 on error.
int camlib_v4l2_modes(const char *path, camlib_v4l2_mode *modes, int capacity, char *error, size_t error_len)
{
    int fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (fd == -1) {
        set_error(error, error_len, "cannot open camera");
        return -1;
    }
    int count = 0;
    struct v4l2_fmtdesc desc;
    memset(&desc, 0, sizeof desc);
    desc.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    while (xioctl(fd, VIDIOC_ENUM_FMT, &desc) == 0) {
        struct v4l2_frmsizeenum size;
        memset(&size, 0, sizeof size);
        size.pixel_format = desc.pixelformat;
        if (xioctl(fd, VIDIOC_ENUM_FRAMESIZES, &size) == 0) {
            if (size.type == V4L2_FRMSIZE_TYPE_DISCRETE) {
                do {
                    enumerate_intervals(fd, desc.pixelformat, size.discrete.width, size.discrete.height, modes,
                                        capacity, &count);
                    ++size.index;
                } while (xioctl(fd, VIDIOC_ENUM_FRAMESIZES, &size) == 0);
            } else {
                struct v4l2_frmsize_stepwise s = size.stepwise;
                enumerate_intervals(fd, desc.pixelformat, s.max_width, s.max_height, modes, capacity, &count);
                enumerate_intervals(fd, desc.pixelformat, s.min_width, s.min_height, modes, capacity, &count);
                if (s.min_width <= 1280 && s.max_width >= 1280 && s.min_height <= 720 && s.max_height >= 720) {
                    enumerate_intervals(fd, desc.pixelformat, 1280, 720, modes, capacity, &count);
                }
            }
        } else {
            // No size enumeration: report the current size for this format.
            struct v4l2_format fmt;
            memset(&fmt, 0, sizeof fmt);
            fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
            if (xioctl(fd, VIDIOC_G_FMT, &fmt) == 0) {
                enumerate_intervals(fd, desc.pixelformat, fmt.fmt.pix.width, fmt.fmt.pix.height, modes, capacity,
                                    &count);
            }
        }
        ++desc.index;
    }
    close(fd);
    return count;
}

void camlib_v4l2_close(camlib_v4l2 *camera)
{
    if (!camera) {
        return;
    }
    if (camera->streaming) {
        enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        xioctl(camera->fd, VIDIOC_STREAMOFF, &type);
    }
    for (uint32_t i = 0; i < camera->count; ++i) {
        if (camera->start[i] && camera->start[i] != MAP_FAILED) {
            munmap(camera->start[i], camera->length[i]);
        }
    }
    if (camera->fd != -1) {
        close(camera->fd);
    }
    free(camera);
}

// Opens `path` in `mode` (updated to what the driver chose) and starts streaming.
// `full_range` reports whether YUV and grey samples span 0-255 rather than 16-235.
camlib_v4l2 *camlib_v4l2_open(const char *path, camlib_v4l2_mode *mode, uint32_t *bytes_per_line, int *full_range,
                              char *error, size_t error_len)
{
    camlib_v4l2 *camera = calloc(1, sizeof *camera);
    if (!camera) {
        set_error(error, error_len, "out of memory");
        return NULL;
    }
    camera->fd = open(path, O_RDWR | O_NONBLOCK | O_CLOEXEC);
    if (camera->fd == -1) {
        set_error(error, error_len, "cannot open camera");
        goto fail;
    }

    struct v4l2_format fmt;
    memset(&fmt, 0, sizeof fmt);
    fmt.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (xioctl(camera->fd, VIDIOC_G_FMT, &fmt) == -1) {
        set_error(error, error_len, "cannot read camera format");
        goto fail;
    }
    fmt.fmt.pix.width = mode->width;
    fmt.fmt.pix.height = mode->height;
    fmt.fmt.pix.pixelformat = mode->fourcc;
    fmt.fmt.pix.field = V4L2_FIELD_ANY;
    if (xioctl(camera->fd, VIDIOC_S_FMT, &fmt) == -1) {
        set_error(error, error_len, "camera rejected the format");
        goto fail;
    }
    mode->width = fmt.fmt.pix.width;
    mode->height = fmt.fmt.pix.height;
    mode->fourcc = fmt.fmt.pix.pixelformat;
    *bytes_per_line = fmt.fmt.pix.bytesperline;
    // V4L2's default quantization is full range only for RGB and the JPEG colorspace.
    *full_range = fmt.fmt.pix.quantization == V4L2_QUANTIZATION_FULL_RANGE ||
                  (fmt.fmt.pix.quantization == V4L2_QUANTIZATION_DEFAULT &&
                   fmt.fmt.pix.colorspace == V4L2_COLORSPACE_JPEG);

    struct v4l2_streamparm parm;
    memset(&parm, 0, sizeof parm);
    parm.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (xioctl(camera->fd, VIDIOC_G_PARM, &parm) == 0) {
        if (parm.parm.capture.capability & V4L2_CAP_TIMEPERFRAME) {
            parm.parm.capture.timeperframe.numerator = mode->numerator;
            parm.parm.capture.timeperframe.denominator = mode->denominator;
            // A refused rate leaves the driver's default, which still streams.
            xioctl(camera->fd, VIDIOC_S_PARM, &parm);
        }
        mode->numerator = parm.parm.capture.timeperframe.numerator;
        mode->denominator = parm.parm.capture.timeperframe.denominator;
    }

    struct v4l2_requestbuffers req;
    memset(&req, 0, sizeof req);
    req.count = CAMLIB_BUFFERS;
    req.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    req.memory = V4L2_MEMORY_MMAP;
    if (xioctl(camera->fd, VIDIOC_REQBUFS, &req) == -1) {
        set_error(error, error_len, "camera refused memory-mapped buffers");
        goto fail;
    }
    if (req.count < 2) {
        errno = ENOMEM;
        set_error(error, error_len, "camera provided fewer than two buffers");
        goto fail;
    }
    camera->count = req.count < CAMLIB_BUFFERS ? req.count : CAMLIB_BUFFERS;
    for (uint32_t i = 0; i < camera->count; ++i) {
        struct v4l2_buffer buf;
        memset(&buf, 0, sizeof buf);
        buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
        buf.memory = V4L2_MEMORY_MMAP;
        buf.index = i;
        if (xioctl(camera->fd, VIDIOC_QUERYBUF, &buf) == -1) {
            set_error(error, error_len, "cannot query camera buffer");
            goto fail;
        }
        camera->length[i] = buf.length;
        camera->start[i] = mmap(NULL, buf.length, PROT_READ | PROT_WRITE, MAP_SHARED, camera->fd, buf.m.offset);
        if (camera->start[i] == MAP_FAILED) {
            set_error(error, error_len, "cannot map camera buffer");
            goto fail;
        }
        if (xioctl(camera->fd, VIDIOC_QBUF, &buf) == -1) {
            set_error(error, error_len, "cannot queue camera buffer");
            goto fail;
        }
    }

    enum v4l2_buf_type type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    if (xioctl(camera->fd, VIDIOC_STREAMON, &type) == -1) {
        set_error(error, error_len, "cannot start camera stream");
        goto fail;
    }
    camera->streaming = 1;
    return camera;

fail:
    camlib_v4l2_close(camera);
    return NULL;
}

// Returns 1 after delivering a frame, 0 on timeout or a skipped corrupt frame, -1 on error and
// -2 when the device is gone.
int camlib_v4l2_read(camlib_v4l2 *camera, int timeout_ms, camlib_v4l2_frame_cb frame_cb, void *ctx, char *error,
                     size_t error_len)
{
    struct pollfd pfd = {.fd = camera->fd, .events = POLLIN};
    int ready = poll(&pfd, 1, timeout_ms);
    if (ready == 0 || (ready == -1 && errno == EINTR)) {
        return 0;
    }
    if (ready == -1) {
        set_error(error, error_len, "camera poll failed");
        return -1;
    }
    if (pfd.revents & (POLLERR | POLLHUP | POLLNVAL)) {
        errno = ENODEV;
        set_error(error, error_len, "camera stopped responding");
        return -2;
    }

    struct v4l2_buffer buf;
    memset(&buf, 0, sizeof buf);
    buf.type = V4L2_BUF_TYPE_VIDEO_CAPTURE;
    buf.memory = V4L2_MEMORY_MMAP;
    if (xioctl(camera->fd, VIDIOC_DQBUF, &buf) == -1) {
        if (errno == EAGAIN) {
            return 0;
        }
        set_error(error, error_len, "cannot dequeue camera frame");
        return errno == ENODEV ? -2 : -1;
    }
    int delivered = 0;
    if (buf.index < camera->count && !(buf.flags & V4L2_BUF_FLAG_ERROR)) {
        size_t used = buf.bytesused ? buf.bytesused : camera->length[buf.index];
        if (used > camera->length[buf.index]) {
            used = camera->length[buf.index];
        }
        frame_cb(ctx, camera->start[buf.index], used);
        delivered = 1;
    }
    if (xioctl(camera->fd, VIDIOC_QBUF, &buf) == -1) {
        set_error(error, error_len, "cannot requeue camera frame");
        return errno == ENODEV ? -2 : -1;
    }
    return delivered;
}
