#include "camera.h"
#include <vector>
#include <cstring>
#include <string>

struct libcamera_stream_set {
    std::vector<libcamera::Stream *> streams;
};

extern "C" {

void libcamera_camera_configuration_destroy(libcamera_camera_configuration_t* config) {
    delete config;
}

size_t libcamera_camera_configuration_size(const libcamera_camera_configuration_t* config) {
    return config->size();
}

libcamera_stream_configuration_t *libcamera_camera_configuration_at(libcamera_camera_configuration_t* config, size_t index) {
    if (config->size() > index) {
        return &config->at(index);
    } else {
        return nullptr;
    }
}

libcamera_camera_configuration_status_t libcamera_camera_configuration_validate(libcamera_camera_configuration_t* config) {
    return config->validate();
}

libcamera_orientation_t libcamera_camera_configuration_get_orientation(const libcamera_camera_configuration_t* config) {
    return config->orientation;
}

void libcamera_camera_configuration_set_orientation(libcamera_camera_configuration_t* config, libcamera_orientation_t orientation) {
    config->orientation = orientation;
}

libcamera_sensor_configuration_t *libcamera_camera_configuration_get_sensor_configuration(const libcamera_camera_configuration_t* config) {
    if (!config->sensorConfig.has_value()) {
        return nullptr;
    }
    return new libcamera_sensor_configuration_t(config->sensorConfig.value());
}

libcamera_stream_configuration_t *libcamera_camera_configuration_add_configuration(libcamera_camera_configuration_t *config) {
    libcamera::StreamConfiguration cfg;
    config->addConfiguration(cfg);
    if (config->size() == 0)
        return nullptr;
    return &config->at(config->size() - 1);
}

libcamera_stream_configuration_t *libcamera_camera_configuration_add_configuration_from(
    libcamera_camera_configuration_t *config,
    const libcamera_stream_configuration_t *src) {
    if (!src)
        return nullptr;
    libcamera::StreamConfiguration cfg = *src;
    config->addConfiguration(cfg);
    if (config->size() == 0)
        return nullptr;
    return &config->at(config->size() - 1);
}

char *libcamera_camera_configuration_to_string(const libcamera_camera_configuration_t *config) {
    std::string out;
    for (size_t i = 0; i < config->size(); ++i) {
        out += config->at(i).toString();
        if (i + 1 < config->size())
            out += " ";
    }
    return ::strdup(out.c_str());
}

libcamera_camera_t* libcamera_camera_copy(libcamera_camera_t *cam) {
    const libcamera_camera_t& ptr = *cam;
    return new libcamera_camera_t(ptr);
}

void libcamera_camera_destroy(libcamera_camera_t *cam) {
    delete cam;
}

const char *libcamera_camera_id(const libcamera_camera_t *cam) {
    return cam->get()->id().c_str();
}

libcamera_callback_handle_t *libcamera_camera_request_completed_connect(libcamera_camera_t *cam, libcamera_request_completed_cb_t *callback, void *data) {
    libcamera_callback_handle_t *handle = new libcamera_callback_handle_t {};

    cam->get()->requestCompleted.connect(handle, [=](libcamera::Request *request) {
        callback(data, request);
    });

    return handle;
}

void libcamera_camera_request_completed_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle) {
    cam->get()->requestCompleted.disconnect(handle);
    delete handle;
}

libcamera_callback_handle_t *libcamera_camera_buffer_completed_connect(libcamera_camera_t *cam, libcamera_buffer_completed_cb_t *callback, void *data) {
    libcamera_callback_handle_t *handle = new libcamera_callback_handle_t {};

    cam->get()->bufferCompleted.connect(handle, [=](libcamera::Request *request, libcamera::FrameBuffer *buffer) {
        callback(data, request, buffer);
    });

    return handle;
}

void libcamera_camera_buffer_completed_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle) {
    cam->get()->bufferCompleted.disconnect(handle);
    delete handle;
}

libcamera_callback_handle_t *libcamera_camera_disconnected_connect(libcamera_camera_t *cam, libcamera_disconnected_cb_t *callback, void *data) {
    libcamera_callback_handle_t *handle = new libcamera_callback_handle_t {};

    cam->get()->disconnected.connect(handle, [=]() {
        callback(data);
    });

    return handle;
}

void libcamera_camera_disconnected_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle) {
    cam->get()->disconnected.disconnect(handle);
    delete handle;
}

int libcamera_camera_acquire(libcamera_camera_t *cam) {
    return cam->get()->acquire();
}

int libcamera_camera_release(libcamera_camera_t *cam) {
    return cam->get()->release();
}

const libcamera_control_info_map_t *libcamera_camera_controls(const libcamera_camera_t *cam) {
    return &cam->get()->controls();
}

const libcamera_control_list_t *libcamera_camera_properties(const libcamera_camera_t *cam) {
    return &cam->get()->properties();
}

const libcamera_stream_set_t *libcamera_camera_streams(const libcamera_camera_t *cam) {
    auto wrapper = new libcamera_stream_set_t();
    const auto &set = cam->get()->streams();
    wrapper->streams.insert(wrapper->streams.end(), set.begin(), set.end());
    return wrapper;
}

libcamera_camera_configuration_t *libcamera_camera_generate_configuration(libcamera_camera_t *cam, const enum libcamera_stream_role *roles, size_t role_count) {
    std::vector<libcamera::StreamRole> roles_vec((libcamera::StreamRole*)roles, (libcamera::StreamRole*)roles + role_count);
    return cam->get()->generateConfiguration(roles_vec).release();
}

int libcamera_camera_configure(libcamera_camera_t *cam, libcamera_camera_configuration_t *config) {
    return cam->get()->configure(config);
}

libcamera_request_t *libcamera_camera_create_request(libcamera_camera_t *cam, uint64_t cookie) {
    return cam->get()->createRequest(cookie).release();
}

int libcamera_camera_queue_request(libcamera_camera_t *cam, libcamera_request_t *request) {
    return cam->get()->queueRequest(request);
}

int libcamera_camera_start(libcamera_camera_t *cam, const libcamera_control_list_t *controls) {
    return cam->get()->start(controls);
}

int libcamera_camera_stop(libcamera_camera_t *cam) {
    return cam->get()->stop();
}

libcamera_sensor_configuration_t *libcamera_sensor_configuration_create()
{
    return new libcamera_sensor_configuration_t();
}

void libcamera_sensor_configuration_set_bit_depth(libcamera_sensor_configuration_t *config, unsigned int bit_depth)
{
    config->bitDepth = bit_depth;
}

bool libcamera_sensor_configuration_is_valid(const libcamera_sensor_configuration_t *config)
{
    return config->isValid();
}

unsigned int libcamera_sensor_configuration_get_bit_depth(const libcamera_sensor_configuration_t *config)
{
    return config->bitDepth;
}

libcamera_size_t libcamera_sensor_configuration_get_output_size(const libcamera_sensor_configuration_t *config)
{
    return config->outputSize;
}

libcamera_rectangle_t libcamera_sensor_configuration_get_analog_crop(const libcamera_sensor_configuration_t *config)
{
    return config->analogCrop;
}

void libcamera_sensor_configuration_get_binning(const libcamera_sensor_configuration_t *config, unsigned int *x, unsigned int *y)
{
    if (x)
        *x = config->binning.binX;
    if (y)
        *y = config->binning.binY;
}

void libcamera_sensor_configuration_get_skipping(const libcamera_sensor_configuration_t *config, unsigned int *x_odd_inc, unsigned int *x_even_inc, unsigned int *y_odd_inc, unsigned int *y_even_inc)
{
    if (x_odd_inc)
        *x_odd_inc = config->skipping.xOddInc;
    if (x_even_inc)
        *x_even_inc = config->skipping.xEvenInc;
    if (y_odd_inc)
        *y_odd_inc = config->skipping.yOddInc;
    if (y_even_inc)
        *y_even_inc = config->skipping.yEvenInc;
}

void libcamera_sensor_configuration_set_output_size(libcamera_sensor_configuration_t *config, unsigned int width, unsigned int height)
{
    config->outputSize = libcamera::Size(width, height);
}

void libcamera_sensor_configuration_set_analog_crop(libcamera_sensor_configuration_t *config, const libcamera_rectangle_t *crop)
{
    config->analogCrop = *crop;
}

void libcamera_sensor_configuration_set_binning(libcamera_sensor_configuration_t *config, unsigned int x, unsigned int y)
{
    config->binning.binX = x;
    config->binning.binY = y;
}

void libcamera_sensor_configuration_set_skipping(libcamera_sensor_configuration_t *config, unsigned int x_odd_inc, unsigned int x_even_inc, unsigned int y_odd_inc, unsigned int y_even_inc)
{
    config->skipping.xOddInc = x_odd_inc;
    config->skipping.xEvenInc = x_even_inc;
    config->skipping.yOddInc = y_odd_inc;
    config->skipping.yEvenInc = y_even_inc;
}

void libcamera_camera_set_sensor_configuration(libcamera_camera_configuration_t *config, const libcamera_sensor_configuration_t *sensor_config)
{
    config->sensorConfig = *sensor_config;
}

void libcamera_sensor_configuration_destroy(libcamera_sensor_configuration_t *config) {
    delete config;
}

}
