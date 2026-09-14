#ifndef __LIBCAMERA_C_CAMERA__
#define __LIBCAMERA_C_CAMERA__

#include "controls.h"
#include "request.h"
#include "signal.h"
#include "stream.h"
#include "geometry.h"

#include <stddef.h>

enum libcamera_camera_configuration_status {
    LIBCAMERA_CAMERA_CONFIGURATION_STATUS_VALID,
    LIBCAMERA_CAMERA_CONFIGURATION_STATUS_ADJUSTED,
    LIBCAMERA_CAMERA_CONFIGURATION_STATUS_INVALID,
};

typedef void libcamera_request_completed_cb_t(void*, libcamera_request_t*);
typedef void libcamera_buffer_completed_cb_t(void*, libcamera_request_t*, libcamera_framebuffer_t*);
typedef void libcamera_disconnected_cb_t(void*);

/* Mirror libcamera::Orientation (values start at 1 to match EXIF) */
enum libcamera_orientation {
    LIBCAMERA_ORIENTATION_ROTATE_0 = 1,
    LIBCAMERA_ORIENTATION_ROTATE_0_MIRROR,
    LIBCAMERA_ORIENTATION_ROTATE_180,
    LIBCAMERA_ORIENTATION_ROTATE_180_MIRROR,
    LIBCAMERA_ORIENTATION_ROTATE_90_MIRROR,
    LIBCAMERA_ORIENTATION_ROTATE_270,
    LIBCAMERA_ORIENTATION_ROTATE_270_MIRROR,
    LIBCAMERA_ORIENTATION_ROTATE_90,
};

#ifdef __cplusplus
#include <libcamera/camera.h>
#include <libcamera/orientation.h>

typedef libcamera::SensorConfiguration libcamera_sensor_configuration_t;
typedef libcamera::CameraConfiguration libcamera_camera_configuration_t;
typedef libcamera::CameraConfiguration::Status libcamera_camera_configuration_status_t;
typedef std::shared_ptr<libcamera::Camera> libcamera_camera_t;
typedef libcamera::Orientation libcamera_orientation_t;

static_assert(static_cast<int>(libcamera::Orientation::Rotate0) == LIBCAMERA_ORIENTATION_ROTATE_0);
static_assert(static_cast<int>(libcamera::Orientation::Rotate0Mirror) == LIBCAMERA_ORIENTATION_ROTATE_0_MIRROR);
static_assert(static_cast<int>(libcamera::Orientation::Rotate180) == LIBCAMERA_ORIENTATION_ROTATE_180);
static_assert(static_cast<int>(libcamera::Orientation::Rotate180Mirror) == LIBCAMERA_ORIENTATION_ROTATE_180_MIRROR);
static_assert(static_cast<int>(libcamera::Orientation::Rotate90Mirror) == LIBCAMERA_ORIENTATION_ROTATE_90_MIRROR);
static_assert(static_cast<int>(libcamera::Orientation::Rotate270) == LIBCAMERA_ORIENTATION_ROTATE_270);
static_assert(static_cast<int>(libcamera::Orientation::Rotate270Mirror) == LIBCAMERA_ORIENTATION_ROTATE_270_MIRROR);
static_assert(static_cast<int>(libcamera::Orientation::Rotate90) == LIBCAMERA_ORIENTATION_ROTATE_90);

extern "C" {
#else
typedef enum libcamera_camera_configuration_status libcamera_camera_configuration_status_t;
typedef struct libcamera_camera_configuration_t libcamera_camera_configuration_t;
typedef struct libcamera_sensor_configuration_t libcamera_sensor_configuration_t;
typedef struct libcamera_camera_t libcamera_camera_t;
typedef enum libcamera_orientation libcamera_orientation_t;
#endif

void libcamera_camera_configuration_destroy(libcamera_camera_configuration_t* config);
size_t libcamera_camera_configuration_size(const libcamera_camera_configuration_t* config);
libcamera_stream_configuration_t *libcamera_camera_configuration_at(libcamera_camera_configuration_t* config, size_t index);
libcamera_camera_configuration_status_t libcamera_camera_configuration_validate(libcamera_camera_configuration_t* config);
libcamera_orientation_t libcamera_camera_configuration_get_orientation(const libcamera_camera_configuration_t* config);
void libcamera_camera_configuration_set_orientation(libcamera_camera_configuration_t* config, libcamera_orientation_t orientation);
libcamera_sensor_configuration_t *libcamera_camera_configuration_get_sensor_configuration(const libcamera_camera_configuration_t* config);
libcamera_stream_configuration_t *libcamera_camera_configuration_add_configuration(libcamera_camera_configuration_t *config);
libcamera_stream_configuration_t *libcamera_camera_configuration_add_configuration_from(libcamera_camera_configuration_t *config, const libcamera_stream_configuration_t *src);
char *libcamera_camera_configuration_to_string(const libcamera_camera_configuration_t *config);

libcamera_camera_t *libcamera_camera_copy(libcamera_camera_t *cam);
void libcamera_camera_destroy(libcamera_camera_t *cam);
const char *libcamera_camera_id(const libcamera_camera_t *cam);
libcamera_callback_handle_t *libcamera_camera_request_completed_connect(libcamera_camera_t *cam, libcamera_request_completed_cb_t *callback, void *data);
void libcamera_camera_request_completed_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle);
libcamera_callback_handle_t *libcamera_camera_buffer_completed_connect(libcamera_camera_t *cam, libcamera_buffer_completed_cb_t *callback, void *data);
void libcamera_camera_buffer_completed_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle);
libcamera_callback_handle_t *libcamera_camera_disconnected_connect(libcamera_camera_t *cam, libcamera_disconnected_cb_t *callback, void *data);
void libcamera_camera_disconnected_disconnect(libcamera_camera_t *cam, libcamera_callback_handle_t *handle);
int libcamera_camera_acquire(libcamera_camera_t *cam);
int libcamera_camera_release(libcamera_camera_t *cam);
const libcamera_control_info_map_t *libcamera_camera_controls(const libcamera_camera_t *cam);
const libcamera_control_list_t *libcamera_camera_properties(const libcamera_camera_t *cam);
const libcamera_stream_set_t *libcamera_camera_streams(const libcamera_camera_t *cam);
libcamera_camera_configuration_t *libcamera_camera_generate_configuration(libcamera_camera_t *cam, const enum libcamera_stream_role *roles, size_t role_count);
int libcamera_camera_configure(libcamera_camera_t *cam, libcamera_camera_configuration_t *config);
libcamera_request_t *libcamera_camera_create_request(libcamera_camera_t *cam, uint64_t cookie);
int libcamera_camera_queue_request(libcamera_camera_t *cam, libcamera_request_t *request);
int libcamera_camera_start(libcamera_camera_t *cam, const libcamera_control_list_t *controls);
int libcamera_camera_stop(libcamera_camera_t *cam);

libcamera_sensor_configuration_t *libcamera_sensor_configuration_create();
void libcamera_sensor_configuration_destroy(libcamera_sensor_configuration_t *config);
bool libcamera_sensor_configuration_is_valid(const libcamera_sensor_configuration_t *config);
unsigned int libcamera_sensor_configuration_get_bit_depth(const libcamera_sensor_configuration_t *config);
libcamera_size_t libcamera_sensor_configuration_get_output_size(const libcamera_sensor_configuration_t *config);
libcamera_rectangle_t libcamera_sensor_configuration_get_analog_crop(const libcamera_sensor_configuration_t *config);
void libcamera_sensor_configuration_get_binning(const libcamera_sensor_configuration_t *config, unsigned int *x, unsigned int *y);
void libcamera_sensor_configuration_get_skipping(const libcamera_sensor_configuration_t *config, unsigned int *x_odd_inc, unsigned int *x_even_inc, unsigned int *y_odd_inc, unsigned int *y_even_inc);
void libcamera_sensor_configuration_set_bit_depth(libcamera_sensor_configuration_t *config, unsigned int bit_depth);
void libcamera_sensor_configuration_set_output_size(libcamera_sensor_configuration_t *config, unsigned int width, unsigned int height);
void libcamera_sensor_configuration_set_analog_crop(libcamera_sensor_configuration_t *config, const libcamera_rectangle_t *crop);
void libcamera_sensor_configuration_set_binning(libcamera_sensor_configuration_t *config, unsigned int x, unsigned int y);
void libcamera_sensor_configuration_set_skipping(libcamera_sensor_configuration_t *config, unsigned int x_odd_inc, unsigned int x_even_inc, unsigned int y_odd_inc, unsigned int y_even_inc);
void libcamera_camera_set_sensor_configuration(libcamera_camera_configuration_t *config, const libcamera_sensor_configuration_t *sensor_config);

#ifdef __cplusplus
}
#endif

#endif
