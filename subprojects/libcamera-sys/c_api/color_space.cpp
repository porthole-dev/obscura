#include "color_space.h"

#include <cstring>

extern "C" {

libcamera_color_space_t libcamera_color_space_make(enum libcamera_color_space_primaries primaries,
                                                   enum libcamera_color_space_transfer_function tf,
                                                   enum libcamera_color_space_ycbcr_encoding ycbcr,
                                                   enum libcamera_color_space_range range) {
    return libcamera::ColorSpace{
        static_cast<libcamera::ColorSpace::Primaries>(primaries),
        static_cast<libcamera::ColorSpace::TransferFunction>(tf),
        static_cast<libcamera::ColorSpace::YcbcrEncoding>(ycbcr),
        static_cast<libcamera::ColorSpace::Range>(range),
    };
}

libcamera_color_space_t libcamera_color_space_raw() {
    return libcamera::ColorSpace::Raw;
}

libcamera_color_space_t libcamera_color_space_srgb() {
    return libcamera::ColorSpace::Srgb;
}

libcamera_color_space_t libcamera_color_space_sycc() {
    return libcamera::ColorSpace::Sycc;
}

libcamera_color_space_t libcamera_color_space_smpte170m() {
    return libcamera::ColorSpace::Smpte170m;
}

libcamera_color_space_t libcamera_color_space_rec709() {
    return libcamera::ColorSpace::Rec709;
}

libcamera_color_space_t libcamera_color_space_rec2020() {
    return libcamera::ColorSpace::Rec2020;
}

char *libcamera_color_space_to_string(const libcamera_color_space_t *color_space) {
    if (!color_space) {
        return nullptr;
    }
    return strdup(color_space->toString().c_str());
}

bool libcamera_color_space_from_string(const char *str, libcamera_color_space_t *out) {
    if (!str || !out) {
        return false;
    }
    auto cs = libcamera::ColorSpace::fromString(std::string(str));
    if (!cs.has_value())
        return false;
    *out = cs.value();
    return true;
}

bool libcamera_color_space_adjust(libcamera_color_space_t *color_space, const libcamera_pixel_format_t *pixel_format) {
    if (!color_space || !pixel_format) {
        return false;
    }
    return color_space->adjust(*pixel_format);
}

}
