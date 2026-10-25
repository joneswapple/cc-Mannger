//! CC Manager desktop shell: runs the ccm-core proxy in-process, shows the
//! React UI, and lives in the system tray.

use ccm_core::{config::Config, http::Status, Core};
use tauri::{
    menu::{Menu, MenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent,
};

#[tauri::command]
fn core_status(core: State<'_, Core>) -> Result<Status, String> {
    core.status().map_err(|e| format!("{e:#}"))
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = app.path().app_config_dir()?.join("config.json");
    let cfg = Config::load(Some(&config_path))?;
    let (core, startup) = Core::open(cfg)?;
    if let Some(id) = startup.imported_account {
        tracing::info!("imported Claude Code credentials as upstream account #{id}");
    }
    if let Some(e) = &startup.import_error {
        tracing::warn!("no Claude Code credentials imported: {e}");
    }
    let server = core.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(e) = ccm_core::http::serve(server, std::future::pending()).await {
            tracing::error!("proxy server stopped: {e:#}");
        }
    });
    app.manage(core);

    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let menu = Menu::with_items(app, &[&show, &quit])?;
    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or("missing app icon")?,
        )
        .tooltip("CC Manager")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id.as_ref() {
            "show" => show_main(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                show_main(tray.app_handle());
            }
        })
        .build(app)?;
    Ok(())
}

pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    tauri::Builder::default()
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing the window keeps the proxy running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![core_status])
        .run(tauri::generate_context!())
        .expect("error while running CC Manager");
}
