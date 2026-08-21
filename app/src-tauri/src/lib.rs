//! VoxBridge 装配层。
//!
//! 这个 crate 自己不做任何业务判断：它把 Windows 的实现塞进 `vox-core` 的每个
//! 端口，把线程按 ARCHITECTURE.md §6 的拓扑摆好，再把内核事件转成前端认的那一个
//! 事件通道。所有"要不要做、什么时候做"的决定都在内核里。
//!
//! 线程拓扑（§6）：
//! - 主线程：只跑 Tauri 事件循环，不干别的活；
//! - 悬浮窗线程：`vox-overlay-win` 自己起，带自己的 Win32 消息泵；
//! - 热键线程：25 ms 轮询 `GetAsyncKeyState`；
//! - 字幕帧线程：定时 prune + render，把内核的字幕状态推给悬浮窗；
//! - 设备轮询线程：低频枚举设备；
//! - tokio：复用 Tauri 自己那个 runtime，**不另起第二个**；
//! - 每条流水线一个工作线程，由 `PipelineEngine` 自己管。

#![cfg(windows)]
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::sync::Arc;

use tauri::Manager;
use vox_core::ports::Clock;
use vox_core::{PipelineEngine, Runtime};

mod audio;
mod commands;
mod devices;
mod dsp;
mod dto;
mod events;
mod input;
mod net;
mod overlay;
mod persist;
mod state;
mod sys;
mod tray;
mod winminmax;

use state::AppState;

/// 应用入口。`main.rs` 只调这一个函数。
pub fn run() {
    sys::log::init();

    // 隐藏 CLI 模式：带 `--vox-restore-defaults` 时只做默认设备写回就退出，
    // 不构建 Tauri（避免被单实例插件当成「重复启动」吞掉）。
    if vox_audio_win::restore_via_args_if_requested() {
        return;
    }

    let app = tauri::Builder::default()
        // 单实例插件的文档要求：必须是第一个注册的插件。
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            focus_main(app);
        }))
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_autostart::init(
            tauri_plugin_autostart::MacosLauncher::LaunchAgent,
            None,
        ))
        .invoke_handler(tauri::generate_handler![
            commands::snapshot,
            commands::update_settings,
            commands::set_api_key,
            commands::start_pipeline,
            commands::stop_pipeline,
            commands::toggle_pipeline,
            commands::reset_usage,
            commands::reset_usage_model,
            commands::refresh_devices,
            commands::install_virtual_cable,
            commands::uninstall_virtual_cable,
            commands::virtual_cable_blockers,
            commands::set_virtual_cable_multichannel_visible,
            commands::open_dashscope_console,
            commands::open_provider_console,
            commands::open_virtual_cable_website,
            commands::open_virtual_cable_donation,
            commands::quit_app,
        ])
        .setup(|app| {
            let state = assemble(app.handle())?;
            app.manage(state);
            Ok(())
        })
        .build(tauri::generate_context!());

    // 构建失败（含 setup 里 assemble 报错）不能默默 panic：发布构建没有控制台，
    // 用户看到的会是"双击图标什么都没发生"。弹个框把原因摆出来再退。
    let app = match app {
        Ok(app) => app,
        Err(e) => {
            tracing::error!("Tauri 应用构建失败：{e}");
            sys::fatal::alert(
                "VoxBridge 启动失败",
                &format!("初始化时出错，应用无法启动。\n\n{e}"),
            );
            return;
        }
    };

    app.run(|app, event| match event {
        // 关设置窗只是收进托盘，进程继续跑——热键和悬浮窗还得用。
        tauri::RunEvent::WindowEvent {
            label,
            event: tauri::WindowEvent::CloseRequested { api, .. },
            ..
        } if label == "main" => {
            api.prevent_close();
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.hide();
            }
        }
        tauri::RunEvent::Exit => shutdown(app),
        _ => {}
    });
}

/// 把所有实现拼起来。返回的 `AppState` 交给 Tauri 托管。
///
/// 这里的每一步都尽量"失败不致命"：设备枚举、悬浮窗、热键任何一样起不来，都只是
/// 记一条 `Notice` 让界面告诉用户，不把整个应用拖死——用户至少得能进设置窗改配置。
fn assemble(app: &tauri::AppHandle) -> Result<Arc<AppState>, Box<dyn std::error::Error>> {
    let config_dir = app.path().app_config_dir()?;
    // start() 而不是 new()：去抖线程要持一份 Arc 才能保证对象活着（详见 persist.rs）。
    let persist = persist::Persist::start(config_dir);

    // 1. 设置 + 时钟 + Runtime。
    let settings = persist.load_settings();
    let clock: Arc<dyn Clock> = Arc::new(sys::clock::SystemClock::new());
    let runtime = Runtime::new(settings, Arc::clone(&clock));

    // 2. 密钥库。set_secret_store 会顺手把存着的密钥读进来。
    runtime.set_secret_store(Arc::new(sys::secrets::DpapiSecretStore::new(
        persist.secret_path(),
    )));

    // 3. 用量账本。要在挂落盘监听之前灌进去，免得刚读出来就又写一遍。
    runtime.load_usage(persist.load_usage());

    // 4. 窗口先亮出来。后面任何一步失败，用户至少看得见界面。
    if let Some(w) = app.get_webview_window("main") {
        // 透明无边框窗口下，tauri.conf.json 的 minHeight 压不住（实测会被压到
        // ~30px），tauri 的 set_min_size 走 tao 的 subclass 链同样压不到下限。
        // 这里用原生子类，在 WM_GETMINMAXINFO 最底层强制最小高度 38（标题栏高）
        // ——用户不能把窗口拖得比「只剩标签栏」更扁。宽度 640 与配置一致。
        if let Ok(hwnd) = w.hwnd() {
            winminmax::enforce_min_size(hwnd.0, 640, 38);
        }
        if runtime.settings().start_minimized {
            let _ = w.hide();
        } else {
            let _ = w.show();
            let _ = w.set_focus();
        }
    }

    // 5. 流水线引擎。tokio 复用 Tauri 那一个 runtime。
    let tokio_handle = tauri::async_runtime::handle().inner().clone();
    let engine = PipelineEngine::new(
        runtime.clone(),
        vox_core::pipeline::Deps {
            transport: net::transport_factory(tokio_handle),
            capture: audio::capture_factory(),
            playback: audio::playback_factory(),
            denoise: dsp::denoise_factory(),
            resample: dsp::resample_factory(),
        },
    );
    runtime.set_control(Arc::clone(&engine) as Arc<_>);

    let registry = audio::registry();
    let state = Arc::new(AppState::new(
        runtime.clone(),
        Arc::clone(&engine),
        Arc::clone(&registry),
        Arc::clone(&persist),
    ));

    // 6. 纯显示悬浮窗 + 字幕帧线程。悬浮窗永久穿透，不处理按钮或设置命令。
    overlay::start(&state);

    // 7. 设备枚举（首次同步一把，之后低频轮询）。
    devices::start(&state);

    // 8. 事件桥：落盘、推给前端、同步开机自启。
    events::wire(&state, app.clone());

    // 9. 热键。注入 host 会触发一次 rebind，把当前绑定推下去。
    //
    // 放在 events::wire 之后：热键线程一起来就可能立刻回调 `on_hotkey` →
    // `update_settings`，要是那会儿 listener 还没挂上，这次改动就不会被标脏，
    // 也就永远不落盘。中间夹着建 Win32 窗口，窗口不止几微秒。
    match input::start(runtime.clone()) {
        Ok(host) => runtime.set_hotkey_host(host),
        Err(e) => runtime.notify(vox_core::event::Notice::error(format!(
            "全局热键起不来，只能用界面上的开关：{e}"
        ))),
    }

    // 10. 托盘。起不来不致命——设置窗和悬浮窗都还在。
    if let Err(e) = tray::install(app, &state) {
        runtime.notify(vox_core::event::Notice::warning(format!(
            "托盘图标没建起来：{e}"
        )));
    }

    Ok(state)
}

/// 退出前收摊。
///
/// 顺序是有讲究的，**竖旗不等于停下**——所有"还能改账本"的线程必须先真的
/// join 掉，才能 flush，否则它们会在 flush 之后把 `dirty` 重新标脏，那份改动
/// 永远不落盘（静默丢数据）。
///
/// 1. `tray::begin_shutdown()` 头一个：主线程马上要卡在下面的 join 里，事件
///    循环停转，这时候再往主线程投递 `set_checked` 没有任何意义。
/// 2. 热键线程最先停——它是唯一能在退出中途 `toggle()` 拉起新工作线程去开麦的。
/// 3. 设备线程、字幕线程，都是会 emit 事件的。
/// 4. 工作线程（`engine.shutdown()`），再关悬浮窗窗口。
/// 5. 最后 flush。此时没有别的线程能碰账本了。
fn shutdown(app: &tauri::AppHandle) {
    let Some(state) = app.try_state::<Arc<AppState>>() else {
        return;
    };
    let state = state.inner();

    tray::begin_shutdown();
    input::stop();
    devices::stop();
    overlay::stop();
    state.engine.shutdown();
    if let Some(overlay) = state.overlay.get() {
        overlay.shutdown();
    }
    state.persist.flush();
}

/// 第二个实例被拦下时，把已经在跑的那个窗口拉到前面。
fn focus_main(app: &tauri::AppHandle) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.show();
        let _ = w.unminimize();
        let _ = w.set_focus();
    }
}
