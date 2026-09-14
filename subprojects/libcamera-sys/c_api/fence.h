#ifndef __LIBCAMERA_C_FENCE__
#define __LIBCAMERA_C_FENCE__

#ifdef __cplusplus
#include <libcamera/fence.h>

typedef libcamera::Fence libcamera_fence_t;

extern "C" {
#else
typedef struct libcamera_fence libcamera_fence_t;
#endif

libcamera_fence_t *libcamera_fence_from_fd(int fd);
void libcamera_fence_destroy(libcamera_fence_t *fence);
int libcamera_fence_fd(const libcamera_fence_t *fence);

#ifdef __cplusplus
}
#endif

#endif
