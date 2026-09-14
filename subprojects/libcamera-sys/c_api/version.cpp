#include "version.h"

#include <libcamera/camera_manager.h>

extern "C" {

const char *libcamera_version_string() {
    return libcamera::CameraManager::version().c_str();
}

}
