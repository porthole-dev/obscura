#include "stream.h"

#include <libcamera/libcamera.h>
#include <vector>
#include <cstring>

extern "C" {

struct libcamera_stream_set {
    std::vector<libcamera::Stream *> streams;
};

libcamera_pixel_formats_t *libcamera_stream_formats_pixel_formats(const libcamera_stream_formats_t* formats) {
    return new libcamera_pixel_formats_t(std::move(formats->pixelformats()));
}

libcamera_sizes_t *libcamera_stream_formats_sizes(const libcamera_stream_formats_t* formats, const libcamera_pixel_format_t *pixel_format) {
    return new libcamera_sizes_t(std::move(formats->sizes(*pixel_format)));
}

libcamera_size_range_t libcamera_stream_formats_range(const libcamera_stream_formats_t* formats, const libcamera_pixel_format_t *pixel_format) {
    return formats->range(*pixel_format);
}

const libcamera_stream_formats_t *libcamera_stream_configuration_formats(const libcamera_stream_configuration_t *config) {
    return &config->formats();
}

libcamera_stream_t *libcamera_stream_configuration_stream(const libcamera_stream_configuration_t *config) {
    return config->stream();
}

bool libcamera_stream_configuration_has_color_space(const libcamera_stream_configuration_t *config) {
    return config->colorSpace.has_value();
}

libcamera_color_space_t libcamera_stream_configuration_get_color_space(const libcamera_stream_configuration_t *config) {
    return config->colorSpace.value_or(libcamera::ColorSpace::Raw);
}

void libcamera_stream_configuration_set_color_space(libcamera_stream_configuration_t *config, const libcamera_color_space_t *color_space) {
    if (color_space) {
        config->colorSpace = *color_space;
    } else {
        config->colorSpace.reset();
    }
}

char *libcamera_stream_configuration_to_string(const libcamera_stream_configuration_t *config) {
    return ::strdup(config->toString().c_str());
}

const libcamera_stream_configuration_t *libcamera_stream_get_configuration(const libcamera_stream_t *stream) {
    if (!stream)
        return nullptr;
    return &stream->configuration();
}

size_t libcamera_stream_set_size(const libcamera_stream_set_t *set) {
    return set->streams.size();
}

libcamera_stream_t *libcamera_stream_set_get(const libcamera_stream_set_t *set, size_t index) {
    if (index >= set->streams.size())
        return nullptr;
    return set->streams.at(index);
}

void libcamera_stream_set_destroy(libcamera_stream_set_t *set) {
    delete set;
}

}
