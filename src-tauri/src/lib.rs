//! CC Manager desktop shell: runs the ccm-core proxy in-process, shows the
//! React UI, and lives in the system tray (current account and quota,
//! account switching, start/stop, notifications on account trouble).

use std::sync::Mutex;
use std::time::{Duration, Instant};

use ccm_core::{config::Config, events::CoreEvent, Core};
use serde::Serialize;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Manager, State, WindowEvent, Wry,
};
use tauri_plugin_notification::NotificationExt;
use tauri_plugin_opener::OpenerExt;
use tokio::sync::oneshot;

/// In-process admin API router, shared by every UI call.
struct Bridge(axum::Router);

/// The running proxy listener, if any.
#[derive(Default)]
struct Proxy {
    stop: Mutex<Option<oneshot::Sender<()>>>,
}

#[derive(Serialize)]
struct BridgeResponse {
    status: u16,
    body: serde_json::Value,
}

/// The UI's single entry point: runs an `/admin/api/*` request against the
/// in-process router (same handlers as over HTTP, no token).
#[tauri::command]
async fn admin_request(
    app: AppHandle,
    bridge: State<'_, Bridge>,
    method: String,
    path: String,
    body: Option<serde_json::Value>,
) -> Result<BridgeResponse, String> {
    let (status, body) = ccm_core::http::call_local(&bridge.0, &method, &path, body)
        .await
        .map_err(|e| format!("{e:#}"))?;
    if method == "POST" && path.starts_with("/admin/api/accounts") {
        refresh_tray(&app);
    }
    Ok(BridgeResponse { status, body })
}

/// Opens an http(s) URL (the OAuth authorize page) in the system browser.
#[tauri::command]
fn open_url(app: AppHandle, url: String) -> Result<(), String> {
    if !(url.starts_with("https://") || url.starts_with("http://")) {
        return Err("only http(s) URLs can be opened".into());
    }
    app.opener()
        .open_url(url, None::<&str>)
        .map_err(|e| e.to_string())
}

fn show_main(app: &AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}

fn start_proxy(app: &AppHandle) {
    let proxy = app.state::<Proxy>();
    let mut stop = proxy.stop.lock().unwrap_or_else(|e| e.into_inner());
    if stop.is_some() {
        return;
    }
    let (tx, rx) = oneshot::channel::<()>();
    *stop = Some(tx);
    let core = app.state::<Core>().inner().clone();
    let handle = app.clone();
    tauri::async_runtime::spawn(async move {
        let result = ccm_core::http::serve(core, async {
            let _ = rx.await;
        })
        .await;
        if let Err(e) = result {
            tracing::error!("proxy server stopped: {e:#}");
            notify(&handle, "代理启动失败", &format!("{e:#}"));
        }
        // Listener gone (stopped or failed): reflect it in the tray.
        handle
            .state::<Proxy>()
            .stop
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .take();
        refresh_tray(&handle);
    });
}

fn stop_proxy(app: &AppHandle) {
    if let Some(tx) = app
        .state::<Proxy>()
        .stop
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .take()
    {
        let _ = tx.send(());
    }
}

fn proxy_running(app: &AppHandle) -> bool {
    app.state::<Proxy>()
        .stop
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .is_some()
}

fn pct(v: Option<f64>) -> String {
    v.map_or("—".into(), |v| format!("{}%", (v * 100.0).round()))
}

/// Rebuilds the tray menu and tooltip from current state.
fn refresh_tray(app: &AppHandle) {
    if let Err(e) = try_refresh_tray(app) {
        tracing::warn!("tray refresh: {e:#}");
    }
}

fn try_refresh_tray(app: &AppHandle) -> anyhow::Result<()> {
    let Some(tray) = app.tray_by_id("main") else {
        return Ok(());
    };
    let core = app.state::<Core>();
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)?
        .as_secs() as i64;
    let accounts = core.accounts.list()?;
    let quotas = core.pool.quotas()?;
    let current = core.pool.preview()?;
    let running = proxy_running(app);
    let healthy = accounts
        .iter()
        .filter(|a| a.enabled && a.status == "active" && a.cooldown_left(now) == 0)
        .count();

    let status_text = if running {
        format!("代理运行中 · {}", core.config.listen)
    } else {
        "代理已停止".to_string()
    };
    let current_text = match &current {
        Some(a) => {
            let q = quotas.get(&a.id);
            format!(
                "当前：#{} {} · 5h {} · 7d {}",
                a.id,
                a.label,
                pct(q.and_then(|q| q.five_hour_utilization)),
                pct(q.and_then(|q| q.seven_day_utilization))
            )
        }
        None => "当前：没有可用账号".to_string(),
    };

    let status_item = MenuItem::with_id(app, "status", status_text, false, None::<&str>)?;
    let toggle = MenuItem::with_id(
        app,
        "toggle",
        if running {
            "停止代理"
        } else {
            "启动代理"
        },
        true,
        None::<&str>,
    )?;
    let current_item = MenuItem::with_id(app, "current", current_text, false, None::<&str>)?;
    let switch_items: Vec<MenuItem<Wry>> = accounts
        .iter()
        .filter(|a| a.enabled)
        .map(|a| {
            let mark = if a.status != "active" {
                "（需重新登录）"
            } else if a.cooldown_left(now) > 0 {
                "（冷却中）"
            } else {
                ""
            };
            MenuItem::with_id(
                app,
                format!("pin:{}", a.id),
                format!("#{} {}{mark}", a.id, a.label),
                true,
                None::<&str>,
            )
        })
        .collect::<Result<_, _>>()?;
    let switch_refs: Vec<&dyn tauri::menu::IsMenuItem<Wry>> = switch_items
        .iter()
        .map(|i| i as &dyn tauri::menu::IsMenuItem<Wry>)
        .collect();
    let switch = Submenu::with_items(app, "切换首选账号", !switch_items.is_empty(), &switch_refs)?;
    let show = MenuItem::with_id(app, "show", "显示主窗口", true, None::<&str>)?;
    let quit = MenuItem::with_id(app, "quit", "退出", true, None::<&str>)?;
    let sep1 = PredefinedMenuItem::separator(app)?;
    let sep2 = PredefinedMenuItem::separator(app)?;
    let menu = Menu::with_items(
        app,
        &[
            &status_item,
            &toggle,
            &sep1,
            &current_item,
            &switch,
            &sep2,
            &show,
            &quit,
        ],
    )?;
    tray.set_menu(Some(menu))?;
    tray.set_tooltip(Some(format!(
        "CC Manager · 可用账号 {healthy}/{}{}",
        accounts.len(),
        if running { "" } else { " · 代理已停止" }
    )))?;
    Ok(())
}

fn on_menu(app: &AppHandle, id: &str) {
    match id {
        "show" => show_main(app),
        "quit" => app.exit(0),
        "toggle" => {
            if proxy_running(app) {
                stop_proxy(app);
            } else {
                start_proxy(app);
            }
            refresh_tray(app);
        }
        other => {
            if let Some(id) = other
                .strip_prefix("pin:")
                .and_then(|s| s.parse::<i64>().ok())
            {
                if let Err(e) = app.state::<Core>().accounts.pin(id) {
                    tracing::warn!("pin account {id}: {e:#}");
                }
                refresh_tray(app);
            }
        }
    }
}

fn notify(app: &AppHandle, title: &str, body: &str) {
    if let Err(e) = app.notification().builder().title(title).body(body).show() {
        tracing::warn!("notification: {e}");
    }
}

/// Turns account health events into notifications and tray refreshes.
fn watch_events(app: AppHandle) {
    let mut rx = app.state::<Core>().accounts.events().subscribe();
    tauri::async_runtime::spawn(async move {
        let mut last_all_cooling: Option<Instant> = None;
        loop {
            let event = match rx.recv().await {
                Ok(e) => e,
                Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                Err(_) => break,
            };
            let label = |id: i64| {
                app.state::<Core>()
                    .accounts
                    .get(id)
                    .map(|a| format!("#{id} {}", a.label))
                    .unwrap_or_else(|_| format!("#{id}"))
            };
            match &event {
                CoreEvent::NeedsReauth { account_id, reason } => notify(
                    &app,
                    "账号需要重新登录",
                    &format!("{}：{reason}", label(*account_id)),
                ),
                CoreEvent::CoolingDown {
                    account_id,
                    seconds,
                    ..
                } if *seconds >= 300 => notify(
                    &app,
                    "账号进入冷却",
                    &format!("{} 将冷却约 {} 分钟", label(*account_id), seconds / 60),
                ),
                // At most one of these every ten minutes.
                CoreEvent::AllCooling { retry_after }
                    if last_all_cooling.is_none_or(|t| t.elapsed() > Duration::from_secs(600)) =>
                {
                    last_all_cooling = Some(Instant::now());
                    notify(
                        &app,
                        "所有账号都在冷却",
                        &format!("请求暂时被拒绝，约 {retry_after} 秒后恢复"),
                    );
                }
                _ => {}
            }
            refresh_tray(&app);
        }
    });
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let config_path = app.path().app_config_dir()?.join("config.json");
    let cfg = Config::load(Some(&config_path))?;
    let (core, startup) = Core::open(cfg)?;
    // The desktop UI talks to the core in-process; the admin API (for
    // headless use from other machines) only gets a token when one is
    // configured or set later with `ccm-server admin reset-token`.
    if !core.config.admin.initial_token.is_empty() {
        core.admin.bootstrap(&core.config.admin.initial_token)?;
    }
    if let Some(id) = startup.imported_account {
        tracing::info!("imported Claude Code credentials as upstream account #{id}");
    }
    if let Some(e) = &startup.import_error {
        tracing::warn!("no Claude Code credentials imported: {e}");
    }
    app.manage(Bridge(ccm_core::http::router(core.clone())));
    app.manage(core);
    app.manage(Proxy::default());

    TrayIconBuilder::with_id("main")
        .icon(
            app.default_window_icon()
                .cloned()
                .ok_or("missing app icon")?,
        )
        .tooltip("CC Manager")
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| on_menu(app, event.id.as_ref()))
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

    let handle = app.handle().clone();
    start_proxy(&handle);
    watch_events(handle.clone());
    // Quota and cooldown countdowns change without events; keep the tray
    // roughly current.
    tauri::async_runtime::spawn(async move {
        loop {
            refresh_tray(&handle);
            tokio::time::sleep(Duration::from_secs(30)).await;
        }
    });
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
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .setup(setup)
        .on_window_event(|window, event| {
            // Closing the window keeps the proxy running in the tray.
            if let WindowEvent::CloseRequested { api, .. } = event {
                let _ = window.hide();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![admin_request, open_url])
        .run(tauri::generate_context!())
        .expect("error while running CC Manager");
}
