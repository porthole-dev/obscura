#include "fence.h"
#include <libcamera/base/unique_fd.h>
#include <unistd.h>

extern "C" {

libcamera_fence_t *libcamera_fence_from_fd(int fd) {
    libcamera::UniqueFD ufd(fd);
    if (!ufd.isValid())
        return nullptr;
    return new libcamera::Fence(std::move(ufd));
}

void libcamera_fence_destroy(libcamera_fence_t *fence) {
    delete fence;
}

int libcamera_fence_fd(const libcamera_fence_t *fence) {
    if (!fence)
        return -1;
    return ::dup(fence->fd().get());
}

}
