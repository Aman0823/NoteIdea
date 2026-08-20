//! 窗口显示/隐藏与速记条延迟测量（NFR-2）。
//!
//! 计时全程在 Rust 侧的单调时钟上完成，避免跨进程时钟对齐问题：
//!   起点 = 全局热键回调进入的第一行
//!   终点 = 前端聚焦输入框、且一帧真正绘制完成后回调 `mark_ready`

use std::sync::Mutex;
use std::time::Instant;

use tauri::{AppHandle, Emitter, Manager, State};

pub const QUICK: &str = "quick";
pub const MAIN: &str = "main";
pub const TIME_PICKER: &str = "time-picker";

/// 速记条延迟测量状态。
#[derive(Default)]
pub struct Timing {
    /// 热键触发的时刻；被 `mark_ready` 取走后置空，防止重复计入。
    pending: Option<Instant>,
    /// 历次测量结果（毫秒），按发生顺序。
    samples: Vec<u128>,
}

/// 热键注册失败的键位，供前端补拉（FR-21）。
#[derive(Default)]
pub struct HotkeyFailures(pub Vec<String>);

/// 唤出速记条。已显示则收起（同一热键可切换）。
pub fn show_quick(app: &AppHandle) {
    // 起点必须记在最前面：后面取窗口、判可见性都算在延迟里。
    let started = Instant::now();

    let Some(win) = app.get_webview_window(QUICK) else {
        eprintln!("[quick] 窗口不存在，预热失败");
        return;
    };

    if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
        return;
    }

    if let Some(state) = app.try_state::<Mutex<Timing>>() {
        if let Ok(mut t) = state.lock() {
            // 上一次还没被 mark_ready 取走就被覆盖，说明连按过快，记一笔以免静默漏样本。
            if t.pending.is_some() {
                eprintln!("[latency] 上一次测量未完成即被覆盖，该样本丢弃");
            }
            t.pending = Some(started);
        }
    }

    let _ = win.center();
    let _ = win.show();

    // Windows 上 show() 之后窗口未必已进入前台，此时 set_focus 会被丢掉，
    // 表现为「窗口看得见但敲键盘没反应」。用 always_on_top 抖动强制把它
    // 抬到前台，再要焦点。
    let _ = win.set_always_on_top(true);
    let _ = win.set_focus();

    // 通知前端清空内容、聚焦输入框，并在焦点确实到位后回调 mark_ready。
    // emit_to 定向到 quick 窗口；win.emit 在 Tauri 2 里是广播。
    let _ = app.emit_to(QUICK, "quick:show", ());
}

/// 唤出主窗口（FR-20）。
pub fn show_main(app: &AppHandle) {
    if let Some(win) = app.get_webview_window(MAIN) {
        let _ = win.show();
        let _ = win.unminimize();
        let _ = win.set_focus();
    }
}

/// 前端在**键盘焦点确实落到输入框**之后调用。返回本次延迟（毫秒）。
///
/// `frames` 是前端等到焦点所用的帧数。它是个诊断信号：
/// 1-2 帧说明焦点随窗口一起到位；几十帧说明 WebView 迟迟拿不到焦点。
#[tauri::command]
pub fn mark_ready(app: AppHandle, frames: u32, timing: State<'_, Mutex<Timing>>) -> Option<u128> {
    let mut t = timing.lock().ok()?;
    let started = t.pending.take()?;
    let ms = started.elapsed().as_millis();
    t.samples.push(ms);
    let n = t.samples.len();
    drop(t);

    println!("[latency] #{n} 热键→可输入 {ms} ms（等焦点 {frames} 帧）");
    let _ = app.emit_to(MAIN, "timings:changed", ());
    Some(ms)
}

#[tauri::command]
pub fn timings(timing: State<'_, Mutex<Timing>>) -> Vec<u128> {
    timing.lock().map(|t| t.samples.clone()).unwrap_or_default()
}

#[tauri::command]
pub fn hide_quick(app: AppHandle) {
    if let Some(win) = app.get_webview_window(QUICK) {
        let _ = win.hide();
    }
}

/// 预热窗口的前端已完成首帧，说明 D22 的预热真的生效了。
#[tauri::command]
pub fn quick_warmed() {
    println!("[quick] 预热完成，WebView 已就绪");
}

/// 调整速记条窗口高度（输入辅助弹层需要撑开窗口才能显示）
#[tauri::command]
pub fn resize_quick(app: AppHandle, height: f64) {
    if let Some(win) = app.get_webview_window(QUICK) {
        if let Err(e) = win.set_size(tauri::LogicalSize::new(620.0, height)) {
            eprintln!("[quick] 调整高度失败: {e}");
        }
    }
}

/// 打开标记选择器窗口（独立窗口，始终置顶）。
///
/// `kind` 决定选什么（time / repeat / tag / intensity），`request_id` 用来
/// 把结果送回正确的发起方——选中结果是**广播**，速记条和主编辑器都收得到，
/// 不带 id 的话在编辑器里点 chip 会连速记条一起改掉。
///
/// 参数经 URL query 传给页面，不再另开一条 IPC 通道。
#[tauri::command]
pub async fn open_marker_picker(app: AppHandle, kind: String, request_id: String) {
    // 复用同一个窗口：换了发起方就得换 query，所以先关掉旧的再建。
    if let Some(win) = app.get_webview_window(TIME_PICKER) {
        let _ = win.close();
    }

    use tauri::{WebviewUrl, WebviewWindowBuilder};

    let url = format!("time-picker.html?kind={kind}&req={request_id}");
    let title = match kind.as_str() {
        "repeat" => "选择重复规则",
        "tag" => "设置标签",
        "intensity" => "选择提醒强度",
        _ => "选择提醒时间",
    };

    let builder = WebviewWindowBuilder::new(&app, TIME_PICKER, WebviewUrl::App(url.into()))
        .title(title)
        .inner_size(260.0, 420.0)
        .resizable(false)
        .decorations(true)
        .always_on_top(true)
        .visible(false);

    match builder.build() {
        Ok(win) => {
            let _ = win.center();
            let _ = win.show();
            let _ = win.set_focus();
        }
        Err(e) => eprintln!("[marker-picker] 创建窗口失败: {e}"),
    }
}

/// 关闭标记选择器窗口。
///
/// 前端不能直接调 `getCurrentWindow().close()`——capability 只给了
/// `core:default`，不含窗口关闭权限，那条调用会被静默拦截（连异常都不抛，
/// 表现为点确定毫无反应，还能反复点出多个时间）。窗口操作一律走 Rust 侧。
#[tauri::command]
pub fn close_time_picker(app: AppHandle) {
    if let Some(win) = app.get_webview_window(TIME_PICKER) {
        if let Err(e) = win.close() {
            eprintln!("[marker-picker] 关闭失败: {e}");
        }
    }
}

#[tauri::command]
pub fn hotkey_failures(state: State<'_, HotkeyFailures>) -> Vec<String> {
    state.0.clone()
}
