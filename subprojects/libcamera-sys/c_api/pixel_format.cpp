#include "pixel_format.h"

#include <libcamera/libcamera.h>
#include <cstring>

extern "C" {

char *libcamera_pixel_format_str(const libcamera_pixel_format_t *format) {
    return strdup(format->toString().c_str());
}

libcamera_pixel_format_t libcamera_pixel_format_from_str(const char *name) {
    if (!name) {
        return libcamera::PixelFormat();
    }
    return libcamera::PixelFormat::fromString(std::string(name));
}

bool libcamera_pixel_format_is_valid(const libcamera_pixel_format_t *format) {
    return format && format->isValid();
}

void libcamera_pixel_formats_destroy(libcamera_pixel_formats_t *formats) {
    delete formats;
}

size_t libcamera_pixel_formats_size(const libcamera_pixel_formats_t *formats) {
    return formats->size();
}

libcamera_pixel_format_t libcamera_pixel_formats_get(const libcamera_pixel_formats_t *formats, size_t index) {
    return (*formats)[index];
}

}
