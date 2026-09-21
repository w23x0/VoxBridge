//! GTK 悬浮字幕窗：透明、无边框、置顶、鼠标穿透。
//!
//! 跟 Windows 版的**关键差别**：GTK 只能在主线程上用。Windows 那边悬浮窗有自己
//! 的线程和 Win32 消息泵；这里所有窗口操作（建窗、改几何、重画、关窗）都必须在
//! GTK 主线程上——也就是 Tauri 的事件循环线程。所以：
//!
//! - **建窗**：`Overlay::spawn` 要求从主线程调（装配层的 `assemble()` 就在主线程），
//!   不是主线程就直接报错，不装作能行；
//! - **渲染**：帧线程只往邮箱里塞最新一帧，再 `glib::MainContext::invoke` 让主线程
//!   重画。邮箱是"后写覆盖先写"——字幕一秒来几十帧，排队只会画一堆过期帧。
//!
//! GNOME 的 Wayland 会话下 GTK 客户端**不能自定坐标、不能置顶**（协议层就没有
//! 这两样），所以这条路的实际形态是 XWayland：装配层在 Wayland 会话里把
//! `GDK_BACKEND=x11` 设上，整个应用跑在 X11 兼容层里。实测 X11 下 `move()` 与
//! `set_keep_above()` 都生效（见 `docs/PLATFORM_LINUX.md` §2.3）。
//!
//! 鼠标穿透按 `DECISIONS.md` A5 的既定方针：**永久穿透**，不做拖动/缩放交互
//! （Windows 侧后来加了拖动，那属于那边的历史包袱，Linux 这边不做）。

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use gtk::prelude::*;
use vox_core::ports::{PortError, PortResult};
use vox_core::settings::{OverlayGeometry, SubtitleSettings};
use vox_overlay_core::canvas::Canvas;
use vox_overlay_core::layout;
use vox_overlay_core::render::{FrameInput, Renderer};
use vox_overlay_core::text::FontFactory;

use super::mailbox::Mailbox;

// GTK 对象只能在主线程碰，所以放 `thread_local`（`static` 要求 `Sync`，GTK 不是）。
// 全进程只有一个悬浮窗（`DECISIONS.md` A6：两条流水线共用一个窗、两行分色），
// 所以这一份就够。
thread_local! {
    static INNER: RefCell<Option<Inner>> = const { RefCell::new(None) };
}

/// 窗口相关的一切（主线程独占）。
struct Inner {
    window: gtk::Window,
    area: gtk::DrawingArea,
    renderer: Renderer,
    /// 上次渲染用的设置，用来判断要不要重建字体。
    settings: SubtitleSettings,
    dpi: u32,
    mailbox: Arc<Mutex<Mailbox>>,
    /// 画布复用：每帧重算像素，但不重新分配缓冲。
    canvas: Canvas,
    /// 上屏用的像素缓冲，复用；`cairo` 每帧借它的指针，不拥有它。
    pixels: Vec<u8>,
}

/// 悬浮窗。外面（帧线程 / 装配层）只拿这个句柄，调的都是线程安全的操作。
pub struct Overlay {
    mailbox: Arc<Mutex<Mailbox>>,
}

impl Overlay {
    /// 建窗。**必须在 GTK 主线程上调**，否则报错。
    pub fn spawn(settings: &SubtitleSettings, font_factory: FontFactory) -> PortResult<Arc<Self>> {
        let context = glib::MainContext::default();
        if !context.is_owner() {
            return Err(PortError::new(
                "Linux 悬浮窗只能在 GTK 主线程上创建（装配层应当在 assemble() 里调）",
            ));
        }

        let mailbox = Arc::new(Mutex::new(Mailbox::new(settings.clone())));

        let window = gtk::Window::new(gtk::WindowType::Toplevel);
        window.set_title("VoxBridge 字幕");
        window.set_decorated(false);
        window.set_resizable(false);
        window.set_accept_focus(false);
        window.set_focus_on_map(false);
        window.set_skip_taskbar_hint(true);
        window.set_skip_pager_hint(true);
        window.set_app_paintable(true);
        window.set_keep_above(true);
        window.set_type_hint(gtk::gdk::WindowTypeHint::Notification);
        if let Some(visual) =
            gtk::prelude::GtkWindowExt::screen(&window).and_then(|screen| screen.rgba_visual())
        {
            window.set_visual(Some(&visual));
        }

        // 几何在设置里是 u32（内核定的），GTK 要 i32。
        let width = settings.geometry.map_or(880, |g| g.width).max(1) as i32;
        let height = settings.geometry.map_or(170, |g| g.height).max(1) as i32;
        let dpi = dpi_of(&window);
        window.set_default_size(width, height);
        let placement = placement_of(&window, width, height, settings.geometry);
        window.move_(placement.0, placement.1);

        let area = gtk::DrawingArea::new();
        area.set_app_paintable(true);
        window.add(&area);
        area.show();

        // 鼠标穿透：把输入域清空（见 `clear_input_shape`）。
        clear_input_shape(&window);

        let renderer = Renderer::new(settings, dpi, font_factory)?;

        INNER.with(|slot| {
            *slot.borrow_mut() = Some(Inner {
                window: window.clone(),
                area: area.clone(),
                renderer,
                settings: settings.clone(),
                dpi,
                mailbox: Arc::clone(&mailbox),
                canvas: Canvas::new(width, height),
                pixels: Vec::new(),
            });
        });

        // 重画回调：主线程上跑，从邮箱取最新一帧。
        area.connect_draw(|_area, cr| {
            INNER.with(|slot| {
                if let Some(inner) = slot.borrow_mut().as_mut() {
                    inner.paint(cr);
                }
            });
            glib::Propagation::Proceed
        });

        window.show();
        // 窗口可见之后再把输入域清一次：有些后端在 map 时会重置 shape。
        clear_input_shape(&window);

        Ok(Arc::new(Self { mailbox }))
    }

    /// 让主线程重画一次。帧线程调这个，自己**不碰**任何 GTK 对象。
    pub fn request_redraw(&self) {
        glib::MainContext::default().invoke(|| {
            INNER.with(|slot| {
                if let Some(inner) = slot.borrow().as_ref() {
                    inner.area.queue_draw();
                }
            });
        });
    }

    /// 窗口还活着吗（帧线程用它发现"窗被关了"）。
    pub fn is_running(&self) -> bool {
        INNER.with(|slot| slot.borrow().as_ref().is_some())
    }

    /// 关窗。主线程执行；可重复调用。
    pub fn shutdown(&self) {
        glib::MainContext::default().invoke(|| {
            INNER.with(|slot| {
                if let Some(inner) = slot.borrow_mut().take() {
                    inner.window.close();
                }
            });
        });
    }

    /// 邮箱（帧线程写、主线程读）。
    pub fn mailbox(&self) -> &Arc<Mutex<Mailbox>> {
        &self.mailbox
    }
}

impl Inner {
    /// 画一帧。**只在主线程调**（cairo 的 `Context` 是主线程给的）。
    fn paint(&mut self, cr: &gtk::cairo::Context) {
        let (frame, visible, settings) = {
            let mailbox = match self.mailbox.lock() {
                Ok(mailbox) => mailbox,
                Err(_) => return,
            };
            (
                mailbox.frame.clone(),
                mailbox.visible,
                mailbox.settings.clone(),
            )
        };

        if !visible {
            self.clear(cr);
            return;
        }

        // 设置变了（字体/字号/底衬透明度）先同步字体。
        if settings != self.settings {
            if let Err(e) = self.renderer.sync_font(&settings, self.dpi) {
                tracing::warn!("换字体失败，继续用旧的：{}", e.message);
            }
            self.settings = settings.clone();
        }

        let Some(frame) = frame else {
            self.clear(cr);
            return;
        };

        // 视口就是**窗口当前尺寸**：窗口高度由用户设置（`subtitle.geometry`）决定，
        // 不跟着内容变——这条跟 Windows 侧一致（那边也只有用户拖动才会改 `rect.h`）。
        // 内容超长时由画布裁剪/左侧滚动处理，见 `vox-overlay-core::layout`。
        let width = self.area.allocated_width().max(1);
        let height = self.area.allocated_height().max(1);
        let input = FrameInput {
            frame: &frame,
            settings: &settings,
            client_width: width,
            client_height: height,
            dpi: self.dpi,
        };
        self.renderer.draw(&mut self.canvas, &input);

        // 像素直接借给 cairo：`create_for_data_unsafe` 不接管所有权，
        // 而 surface 在函数结束前就 drop，`self.pixels` 活得比它久——安全。
        let stride = width * 4;
        self.pixels.clear();
        self.pixels.extend_from_slice(self.canvas.bytes());
        let surface = unsafe {
            gtk::cairo::ImageSurface::create_for_data_unsafe(
                self.pixels.as_mut_ptr(),
                gtk::cairo::Format::ARgb32,
                width,
                height,
                stride,
            )
        };
        let surface = match surface {
            Ok(surface) => surface,
            Err(e) => {
                tracing::warn!("建 cairo 表面失败：{e}");
                return;
            }
        };
        if let Err(e) = cr.set_source_surface(&surface, 0.0, 0.0) {
            tracing::warn!("设置绘制源失败：{e}");
            return;
        }
        cr.set_operator(gtk::cairo::Operator::Source);
        let _ = cr.paint();
    }

    /// 没内容/隐藏时画成全透明。
    fn clear(&mut self, cr: &gtk::cairo::Context) {
        cr.set_operator(gtk::cairo::Operator::Source);
        cr.set_source_rgba(0.0, 0.0, 0.0, 0.0);
        let _ = cr.paint();
    }
}

/// 鼠标穿透：把输入域清空。X11 下这就是 `XShapeCombineRegion(ShapeInput)`，
/// 实测窗口不再接任何指针事件（见 docs §2.3）。
///
/// 建窗时要清一次、`show()` 之后还要再清一次（有些后端在 map 时会重置 shape）。
fn clear_input_shape(window: &gtk::Window) {
    if let Some(gdk_window) = window.window() {
        let empty = gtk::cairo::Region::create();
        gdk_window.input_shape_combine_region(&empty, 0, 0);
    }
}

/// 当前 DPI：GTK 的缩放系数 × 96（Windows 侧用的是 `GetDpiForWindow`，语义一致）。
fn dpi_of(window: &gtk::Window) -> u32 {
    let scale = window
        .window()
        .map(|gdk_window| gdk_window.scale_factor())
        .unwrap_or(1)
        .max(1);
    (96 * scale) as u32
}

/// 窗口左上角坐标：用户设过就用设置里的，没设过就底部居中。
fn placement_of(
    window: &gtk::Window,
    width: i32,
    height: i32,
    geometry: Option<OverlayGeometry>,
) -> (i32, i32) {
    if let Some(geometry) = geometry {
        return (geometry.x, geometry.y);
    }
    let work = gtk::prelude::GtkWindowExt::screen(window)
        .and_then(|screen| screen.display().primary_monitor())
        .map(|monitor| {
            let area = monitor.workarea();
            (area.x(), area.y(), area.width(), area.height())
        })
        .unwrap_or((0, 0, 1920, 1080));
    let rect = vox_overlay_core::geom::RectI::new(work.0, work.1, work.2, work.3);
    // 离底边 80px：跟 Windows 侧 `DEFAULT_BOTTOM_MARGIN` 同一个值（照搬旧版）。
    let placed = layout::default_placement(rect, width, height, 80);
    (placed.x, placed.y)
}
