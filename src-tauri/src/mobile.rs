//! The phone app (iOS, Android): it shows the library of a Phototek server
//! (see server/), so the interface talks to it over HTTP and there are no
//! commands here.

#[tauri::mobile_entry_point]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|_app| {
            #[cfg(target_os = "ios")]
            full_screen_page(_app)?;
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running Phototek");
}

/// WKWebView shrinks the page by the safe areas (the notch, the home
/// indicator) unless told not to. The page then fills the screen and keeps
/// clear of them itself, with `env(safe-area-inset-*)`.
#[cfg(target_os = "ios")]
fn full_screen_page(app: &tauri::App) -> tauri::Result<()> {
    use objc2::msg_send;
    use objc2::runtime::AnyObject;
    use tauri::Manager;

    /// UIScrollViewContentInsetAdjustmentBehavior.never
    const NEVER: isize = 2;
    if let Some(window) = app.get_webview_window("main") {
        window.with_webview(|webview| unsafe {
            let wk = webview.inner() as *mut AnyObject;
            let scroll: *mut AnyObject = msg_send![wk, scrollView];
            let _: () = msg_send![scroll, setContentInsetAdjustmentBehavior: NEVER];
        })?;
    }
    Ok(())
}
