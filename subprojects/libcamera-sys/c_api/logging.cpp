#include <iostream>
#include "logging.h"

extern "C" {

int libcamera_log_set_file(const char *path, bool color) {
    return libcamera::logSetFile(path, color);
}

int libcamera_log_set_stream(libcamera_logging_stream_t stream, bool color) {
    std::ostream *ostream = nullptr;
    switch (stream) {
        case LIBCAMERA_LOGGING_STREAM_STDOUT:
            ostream = &std::cout;
            break;
        case LIBCAMERA_LOGGING_STREAM_STDERR:
            ostream = &std::cerr;
            break;
        case LIBCAMERA_LOGGING_STREAM_CUSTOM:
            return -EINVAL;
    }
    return libcamera::logSetStream(ostream, color);
}

int libcamera_log_set_custom_stream(void *ostream, bool color) {
    if (!ostream)
        return -EINVAL;
    auto *os = static_cast<std::ostream *>(ostream);
    return libcamera::logSetStream(os, color);
}

int libcamera_log_set_target(libcamera_logging_target_t target) {
    return libcamera::logSetTarget(target);
}

void libcamera_log_set_level(const char *category, const char *level) {
    libcamera::logSetLevel(category, level);
}

}
