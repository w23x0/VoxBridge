//! 把主窗口的最小尺寸钉死到最底层的窗口过程。
//!
//! 为什么需要这一层：tauri 的 `set_min_size`（以及 conf 里的 `minHeight`）最终
//! 落在 tao 挂的 `SetWindowSubclass` 链上处理，但这条链对「透明 + 无边框」窗口
//! 实测压不到下限（用户量出来是 30px）。不深究 tao 内部，这里用
//! `SetWindowLongPtrW(GWL_WNDPROC)` 在主窗口**最外面**再包一层窗口函数：收到
//! `WM_GETMINMAXINFO` 时，先放原函数（tao 那条 subclass 链）处理，再强制把
//! `ptMinTrackSize` 改成我们要的值；同时兜住 `WM_WINDOWPOSCHANGING`，把绕过
//! `WM_GETMINMAXINFO` 的缩放路径（自定义 resize、程序化 SetWindowPos 等）也钳住。
//!
//! 兼容性：tao 挂的是 `SetWindowSubclass`，我们换的是最底层窗口过程指针，二者互不
//! 覆盖，原函数指针由 `call_orig` 原样接续。

use std::ffi::c_void;
use std::sync::atomic::{AtomicI32, AtomicIsize, Ordering};

use windows::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    UI::{
        HiDpi::GetDpiForWindow,
        WindowsAndMessaging::{
            CallWindowProcW, DefWindowProcW, GetAncestor, SetWindowLongPtrW, GA_ROOT, GWL_WNDPROC,
            MINMAXINFO, SWP_NOSIZE, WINDOWPOS, WM_GETMINMAXINFO, WM_WINDOWPOSCHANGING,
        },
    },
};

/// 窗口函数类型。和 windows crate 的 `WNDPROC` 同型（Option<extern fn>）。
type WndProc = unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT;

/// 换上之前的最底层窗口函数指针。`0` 表示还没装上。
static ORIG_PROC: AtomicIsize = AtomicIsize::new(0);

/// 逻辑(DIP)最小尺寸，`0` 表示不设这一维。
static MIN_W_DIP: AtomicI32 = AtomicI32::new(0);
static MIN_H_DIP: AtomicI32 = AtomicI32::new(0);

/// 给窗口装上「低于这个尺寸就不许再小」的钳制。
///
/// 入口写成裸指针，是为了绕开 tauri 用的 windows 0.61 和工程直接用的 0.62 两种
/// `HWND` 类型互不相通的问题：调用方把 `w.hwnd()?` 的 `.0`（`*mut c_void`）原样
/// 传进来，这里再封装成本 crate 认识的 `HWND`。
pub fn enforce_min_size(raw_hwnd: *mut c_void, min_w_dip: u32, min_h_dip: u32) {
    MIN_W_DIP.store(min_w_dip as i32, Ordering::SeqCst);
    MIN_H_DIP.store(min_h_dip as i32, Ordering::SeqCst);
    if MIN_W_DIP.load(Ordering::SeqCst) == 0 && MIN_H_DIP.load(Ordering::SeqCst) == 0 {
        return;
    }
    unsafe {
        // 用户拖拽缩放 / 系统测算尺寸，窗口过程跑在「顶层」窗口上。tauri 的
        // hwnd() 可能返回的是 WebView2 宿主或子窗口，这里用 GetAncestor 一路
        // 爬回根，确保子类挂在真正会收到 WM_GETMINMAXINFO 的那一层。
        let mut hwnd = HWND(raw_hwnd);
        let root = GetAncestor(hwnd, GA_ROOT);
        if !root.0.is_null() {
            hwnd = root;
        }
        let orig = SetWindowLongPtrW(hwnd, GWL_WNDPROC, wndproc as *const () as isize);
        ORIG_PROC.store(orig, Ordering::SeqCst);
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if msg == WM_GETMINMAXINFO {
        // 先让原函数（tao 的 subclass 链）把 ptMinTrackSize 填上它自己的值，
        // 再强制覆盖成我们的下限。顺序不能反：tao 还要在里面补 max 等。
        let r = call_orig(hwnd, msg, wparam, lparam);

        let dpi_scale = unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 };
        let mmi = &mut *(lparam.0 as *mut MINMAXINFO);
        let min_w = MIN_W_DIP.load(Ordering::SeqCst);
        if min_w > 0 {
            mmi.ptMinTrackSize.x = ((min_w as f32) * dpi_scale).round() as i32;
        }
        let min_h = MIN_H_DIP.load(Ordering::SeqCst);
        if min_h > 0 {
            mmi.ptMinTrackSize.y = ((min_h as f32) * dpi_scale).round() as i32;
        }
        return r;
    }
    if msg == WM_WINDOWPOSCHANGING {
        let r = call_orig(hwnd, msg, wparam, lparam);

        let dpi_scale = unsafe { GetDpiForWindow(hwnd) as f32 / 96.0 };
        let wp = &mut *(lparam.0 as *mut WINDOWPOS);
        if !wp.flags.contains(SWP_NOSIZE) {
            let min_w = MIN_W_DIP.load(Ordering::SeqCst);
            if min_w > 0 {
                let min_w_physical = ((min_w as f32) * dpi_scale).round() as i32;
                if wp.cx < min_w_physical {
                    wp.cx = min_w_physical;
                }
            }
            let min_h = MIN_H_DIP.load(Ordering::SeqCst);
            if min_h > 0 {
                let min_h_physical = ((min_h as f32) * dpi_scale).round() as i32;
                if wp.cy < min_h_physical {
                    wp.cy = min_h_physical;
                }
            }
        }
        return r;
    }
    call_orig(hwnd, msg, wparam, lparam)
}

/// 把消息接回换掉之前的最底层窗口函数。
unsafe fn call_orig(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    let orig = ORIG_PROC.load(Ordering::SeqCst);
    if orig == 0 {
        return DefWindowProcW(hwnd, msg, wparam, lparam);
    }
    let orig_proc: WndProc = unsafe { std::mem::transmute(orig) };
    CallWindowProcW(Some(orig_proc), hwnd, msg, wparam, lparam)
}
