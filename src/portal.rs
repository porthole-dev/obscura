// SPDX-License-Identifier: GPL-3.0-or-later
//! Camera permission through xdg-desktop-portal, the way GNOME apps ask for
//! it, so the grant shows up (and can be revoked) in Settings › Privacy.

use ashpd::desktop::camera::Camera;

use crate::APP_ID;

#[derive(Debug)]
pub enum Access {
    /// With the PipeWire remote when the cameras come through PipeWire.
    Granted(Option<std::os::fd::OwnedFd>),
    Denied,
    /// No portal, or no camera interface on it. Outside a sandbox the app can
    /// still reach the cameras itself, so this is not fatal there.
    Unavailable(String),
}

pub async fn request_access() -> Access {
    // Development only: a nested or headless compositor has no access dialog,
    // and the portal reports that exactly like a user saying no.
    if !ashpd::is_sandboxed() && std::env::var_os("OBSCURA_SKIP_PORTAL").is_some() {
        return Access::Unavailable("OBSCURA_SKIP_PORTAL is set".into());
    }
    // Host apps have no app id of their own; registering gives the portal
    // one to store the permission under.
    if !ashpd::is_sandboxed()
        && let Ok(id) = ashpd::AppID::try_from(APP_ID)
        && let Err(e) = ashpd::register_host_app(id).await
    {
        log::info!("host app registration: {e}");
    }
    let camera = match Camera::new().await {
        Ok(c) => c,
        Err(e) => return Access::Unavailable(e.to_string()),
    };
    match camera.request_access(Default::default()).await.and_then(|r| r.response()) {
        Ok(()) if crate::pipewire::wanted() => match camera.open_pipe_wire_remote(Default::default()).await {
            Ok(fd) => Access::Granted(Some(fd)),
            Err(e) => Access::Unavailable(format!("no PipeWire remote: {e}")),
        },
        Ok(()) => Access::Granted(None),
        Err(ashpd::Error::Response(ashpd::desktop::ResponseError::Cancelled)) => Access::Denied,
        Err(ashpd::Error::Response(_)) => Access::Denied,
        Err(e) => Access::Unavailable(e.to_string()),
    }
}
