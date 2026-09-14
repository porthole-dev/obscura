#include <libcamera/version.h>

#ifdef __cplusplus
extern "C" {
#endif

/* Returns the libcamera version string without requiring a CameraManager instance. */
const char *libcamera_version_string();

#ifdef __cplusplus
}
#endif
