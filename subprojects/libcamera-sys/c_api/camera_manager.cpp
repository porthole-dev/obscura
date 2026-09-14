#include "camera_manager.h"

#include <libcamera/camera_manager.h>
#include <libcamera/base/signal.h>

extern "C" {

libcamera_camera_manager_t *libcamera_camera_manager_create() {
    return new libcamera::CameraManager();
}

void libcamera_camera_manager_destroy(libcamera_camera_manager_t *mgr) {
    delete mgr;
}

int libcamera_camera_manager_start(libcamera_camera_manager_t *mgr) {
    return mgr->start();
}

void libcamera_camera_manager_stop(libcamera_camera_manager_t *mgr) {
    mgr->stop();
}

libcamera_camera_list_t *libcamera_camera_manager_cameras(const libcamera_camera_manager_t *mgr) {
    return new libcamera_camera_list_t(std::move(mgr->cameras()));
}

libcamera_camera_t *libcamera_camera_manager_get_id(libcamera_camera_manager_t *mgr, const char *id) {
    auto camera = mgr->get(std::string(id));

    if (camera == nullptr)
        return NULL;
    else
        return new libcamera_camera_t(camera);
}

const char *libcamera_camera_manager_version(libcamera_camera_manager_t *mgr) {
    return mgr->version().c_str();
}

libcamera_callback_handle_t *libcamera_camera_manager_camera_added_connect(libcamera_camera_manager_t *mgr, libcamera_camera_added_cb_t *callback, void *data) {
    libcamera_callback_handle_t *handle = new libcamera_callback_handle_t {};
    mgr->cameraAdded.connect(handle, [=](std::shared_ptr<libcamera::Camera> cam) {
        auto copy = new std::shared_ptr<libcamera::Camera>(cam);
        callback(data, copy);
    });
    return handle;
}

libcamera_callback_handle_t *libcamera_camera_manager_camera_removed_connect(libcamera_camera_manager_t *mgr, libcamera_camera_removed_cb_t *callback, void *data) {
    libcamera_callback_handle_t *handle = new libcamera_callback_handle_t {};
    mgr->cameraRemoved.connect(handle, [=](std::shared_ptr<libcamera::Camera> cam) {
        auto copy = new std::shared_ptr<libcamera::Camera>(cam);
        callback(data, copy);
    });
    return handle;
}

void libcamera_camera_manager_camera_signal_disconnect(libcamera_camera_manager_t *mgr, libcamera_callback_handle_t *handle) {
    mgr->cameraAdded.disconnect(handle);
    mgr->cameraRemoved.disconnect(handle);
    delete handle;
}

size_t libcamera_camera_list_size(libcamera_camera_list_t *list) {
    return list->size();
}

libcamera_camera_t *libcamera_camera_list_get(libcamera_camera_list_t *list, size_t index) {
    if (list->size() <= index)
        return nullptr;
    else
        return new libcamera_camera_t(list->at(index));
}

void libcamera_camera_list_destroy(libcamera_camera_list_t *list) {
    delete list;
}

}
