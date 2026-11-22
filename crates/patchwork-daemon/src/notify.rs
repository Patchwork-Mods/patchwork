//! Desktop notifications, for tasks to tell the user about something (see
//! [`crate::Cx::notify`]).

use anyhow::Result;
use notify_rust::Notification;

/// The app's identifier (`identifier` in `tauri.conf.json`), which Windows
/// and macOS go by to say who a notification's from.
#[cfg(any(windows, target_os = "macos"))]
const APP_ID: &str = "dev.patchwork.app";

pub async fn show(summary: &str, body: &str) -> Result<()> {
    let mut n = Notification::new();
    n.summary(summary).body(body).appname("Patchwork");
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        // the name the app's icon is installed under (see packaging/nix)
        n.icon("patchwork");
        n.show_async().await?;
    }
    #[cfg(windows)]
    {
        n.app_id(APP_ID);
        tokio::task::spawn_blocking(move || n.show()).await??;
    }
    #[cfg(target_os = "macos")]
    {
        static APP: std::sync::Once = std::sync::Once::new();
        // macOS only lets this be set once, and it's the same every time
        APP.call_once(|| drop(notify_rust::set_application(APP_ID)));
        tokio::task::spawn_blocking(move || n.show()).await??;
    }
    Ok(())
}
