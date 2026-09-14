#ifndef __LIBCAMERA_C_TRANSFORM__
#define __LIBCAMERA_C_TRANSFORM__

#include <stdbool.h>
#include <stdint.h>

#include "camera.h"

struct libcamera_transform {
    uint32_t value;
};

typedef struct libcamera_transform libcamera_transform_t;

#ifdef __cplusplus
#include "camera.h"
#include <libcamera/transform.h>

static_assert(sizeof(libcamera_transform_t) == sizeof(libcamera::Transform));
static_assert(alignof(libcamera_transform_t) == alignof(libcamera::Transform));

extern "C" {
#endif

libcamera_transform_t libcamera_transform_identity();
libcamera_transform_t libcamera_transform_from_rotation(int angle, bool hflip, bool *success);
libcamera_transform_t libcamera_transform_inverse(libcamera_transform_t transform);
libcamera_transform_t libcamera_transform_combine(libcamera_transform_t a, libcamera_transform_t b);
char *libcamera_transform_to_string(libcamera_transform_t transform);
libcamera_transform_t libcamera_transform_between_orientations(libcamera_orientation_t from, libcamera_orientation_t to);
libcamera_orientation_t libcamera_transform_apply_orientation(libcamera_orientation_t orientation, libcamera_transform_t transform);
libcamera_orientation_t libcamera_orientation_from_rotation(int angle, bool *success);
libcamera_transform_t libcamera_transform_hflip();
libcamera_transform_t libcamera_transform_vflip();
libcamera_transform_t libcamera_transform_transpose();
libcamera_transform_t libcamera_transform_or(libcamera_transform_t a, libcamera_transform_t b);
libcamera_transform_t libcamera_transform_and(libcamera_transform_t a, libcamera_transform_t b);
libcamera_transform_t libcamera_transform_xor(libcamera_transform_t a, libcamera_transform_t b);
libcamera_transform_t libcamera_transform_not(libcamera_transform_t t);

#ifdef __cplusplus
}
#endif

#endif
