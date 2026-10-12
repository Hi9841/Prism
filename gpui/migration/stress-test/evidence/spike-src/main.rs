//! Step 2 spike: settles the 8 needs-spike / assumption rows of migration/inventory.tsv
//! on upstream gpui 0.2.2. Throwaway code. Not part of the port.
//!
//! `gpui-spike auto [--ole-inline]` drives the window with real OS input (SendInput)
//! and prints one JSON object per result line. `gpui-spike` alone opens the window
//! for manual checks. `gpui-spike chrome-probe` records the Chromium baseline.

mod chrome_probe;
mod win;

use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::{Duration, Instant};

use gpui::{
    AnyElement, App, Application, Bounds, Context, DispatchPhase, FocusHandle, KeyBinding,
    KeyDownEvent, Keystroke, ListAlignment, ListOffset, ListState, MouseButton, MouseDownEvent,
    MouseMoveEvent, MouseUpEvent, Pixels, Point, ScrollDelta, ScrollWheelEvent, Subscription,
    Task, Timer, Window, WindowBackgroundAppearance, WindowBounds, WindowKind, WindowOptions,
    actions, canvas, div, list, point, prelude::*, px, rems, rgb, rgba, size,
};
use serde_json::{Value, json};
use windows::Win32::UI::Input::KeyboardAndMouse::*;

actions!(spike, [SelectPrev, SelectNext]);

/// lib/types.ts VIEW_ZOOM_LEVELS
const ZOOM_LEVELS: [u32; 9] = [70, 80, 90, 100, 110, 120, 130, 140, 150];
/// Palette.tsx: `Math.hypot(dx, dy) >= 4`
const DRAG_THRESHOLD: f32 = 4.0;
/// Palette.tsx animatePreviewOut
const PREVIEW_LEAVE_MS: u64 = 110;
/// Chromium on Windows: one wheel line = 100/3 px (hypothesis checked by chrome-probe)
const CHROMIUM_PX_PER_LINE: f32 = 100.0 / 3.0;

type Log = Rc<RefCell<Vec<Value>>>;

fn emit(log: &Log, v: Value) {
    println!("{v}");
    log.borrow_mut().push(v);
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum Kind {
    Header,
    App,
    File,
}

#[derive(Clone)]
struct Row {
    id: String,
    title: String,
    kind: Kind,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum DragKind {
    Reorder,
    Category,
    File,
}

#[derive(Clone, Debug)]
struct PointerDrag {
    kind: DragKind,
    source_id: String,
    start: Point<Pixels>,
    pos: Point<Pixels>,
    active: bool,
    target_id: Option<String>,
}

#[derive(Clone, Copy, PartialEq)]
enum OleMode {
    /// From a foreground task, outside every GPUI callback (the proposed rule).
    Deferred,
    /// Directly inside the mouse-move listener (the anti-pattern).
    Inline,
}

struct Spike {
    focus: FocusHandle,
    rows: Vec<Row>,
    list: ListState,
    selected: usize,
    zoom: u32,
    wheel_delta: f32,
    wheel_reset: Option<Task<()>>,
    drag: Option<PointerDrag>,
    pending_point: Option<Point<Pixels>>,
    frame_scheduled: bool,
    leaving: Option<PointerDrag>,
    leaving_task: Option<Task<()>>,
    suppress_click: bool,
    row_bounds: Rc<RefCell<Vec<(usize, Bounds<Pixels>)>>>,
    raw_moves: u32,
    applies: u32,
    clicks: Vec<usize>,
    ole_mode: OleMode,
    ole_task: Option<Task<()>>,
    ole_result: Option<String>,
    frames: Rc<Cell<u64>>,
    manual: bool,
    file_path: PathBuf,
    log: Log,
    _subs: Vec<Subscription>,
}

fn make_rows() -> Vec<Row> {
    let mut rows = Vec::new();
    for (group, count, kind) in [("files", 4, Kind::File), ("apps", 20, Kind::App), ("more", 175, Kind::App)] {
        rows.push(Row { id: group.into(), title: group.to_uppercase(), kind: Kind::Header });
        for n in 0..count {
            rows.push(Row { id: format!("{group}:{n}"), title: format!("{group} item {n}"), kind });
        }
    }
    rows
}

impl Spike {
    fn new(window: &mut Window, cx: &mut Context<Self>, log: Log, ole_mode: OleMode, manual: bool, file_path: PathBuf) -> Self {
        let rows = make_rows();
        let list = ListState::new(rows.len(), ListAlignment::Top, px(200.)).measure_all();
        let focus = cx.focus_handle();
        window.focus(&focus);

        // Ctrl zoom keys: an interceptor runs before action bindings and before
        // every key listener (window.rs dispatch_key_event), like the original's
        // window keydown listener in the capture phase.
        let weak = cx.weak_entity();
        let intercept = cx.intercept_keystrokes(move |event, _window, cx| {
            let handled = weak
                .update(cx, |this, cx| this.zoom_key(&event.keystroke, cx))
                .unwrap_or(false);
            if handled {
                cx.stop_propagation();
            }
        });

        let poll = cx.spawn(async move |this, cx| {
            if !manual {
                return;
            }
            loop {
                Timer::after(Duration::from_secs(1)).await;
                if this.update(cx, |_, cx| cx.notify()).is_err() {
                    return;
                }
            }
        });
        poll.detach();
        Self {
            focus,
            rows,
            list,
            selected: 1,
            zoom: 100,
            wheel_delta: 0.0,
            wheel_reset: None,
            drag: None,
            pending_point: None,
            frame_scheduled: false,
            leaving: None,
            leaving_task: None,
            suppress_click: false,
            row_bounds: Rc::default(),
            raw_moves: 0,
            applies: 0,
            clicks: Vec::new(),
            ole_mode,
            ole_task: None,
            ole_result: None,
            frames: Rc::default(),
            manual,
            file_path,
            log,
            _subs: vec![intercept],
        }
    }

    // ---- zoom (app.tsx:287-344) ---------------------------------------------

    fn step_zoom(&mut self, direction: i32, cx: &mut Context<Self>) {
        let ix = ZOOM_LEVELS.iter().position(|z| *z == self.zoom).unwrap_or(3) as i32;
        let next = ZOOM_LEVELS[(ix + direction).clamp(0, ZOOM_LEVELS.len() as i32 - 1) as usize];
        self.zoom = next;
        cx.notify();
    }

    fn zoom_key(&mut self, ks: &Keystroke, cx: &mut Context<Self>) -> bool {
        let m = ks.modifiers;
        let mut decision = "ignored";
        if m.control && !m.alt && !m.platform {
            // Chromium's event.key is the produced character; GPUI's `key` is the
            // unshifted key name, so `+` arrives as key "=" with shift. Both work here.
            match ks.key.as_str() {
                "0" => {
                    self.zoom = 100;
                    cx.notify();
                    decision = "reset";
                }
                "up" | "+" | "=" => {
                    self.step_zoom(1, cx);
                    decision = "in";
                }
                "down" | "-" | "_" => {
                    self.step_zoom(-1, cx);
                    decision = "out";
                }
                _ => {}
            }
        }
        emit(&self.log, json!({"event":"intercept","key":ks.key,"key_char":ks.key_char,
            "ctrl":m.control,"shift":m.shift,"alt":m.alt,"decision":decision,"zoom":self.zoom}));
        decision != "ignored"
    }

    fn zoom_wheel(&mut self, e: &ScrollWheelEvent, cx: &mut Context<Self>) {
        // Chromium reports wheel deltas in pixels (deltaMode 0). Convert GPUI's
        // Windows line deltas to the same pixels; Chromium's deltaY sign is
        // the opposite of GPUI's.
        // Blink divides by devicePixelRatio, which includes the WebView zoom.
        let zoom = self.zoom as f32 / 100.;
        let delta_y = match e.delta {
            ScrollDelta::Lines(p) => -p.y * CHROMIUM_PX_PER_LINE / zoom,
            ScrollDelta::Pixels(p) => -f32::from(p.y) / zoom,
        };
        emit(&self.log, json!({"event":"ctrl-wheel","gpui_delta":format!("{:?}", e.delta),"chromium_px":delta_y}));
        if delta_y == 0.0 {
            return;
        }
        if self.wheel_delta != 0.0 && delta_y.signum() != self.wheel_delta.signum() {
            self.wheel_delta = 0.0;
        }
        self.wheel_delta += delta_y;
        if self.wheel_delta.abs() >= 40.0 {
            self.step_zoom(if self.wheel_delta < 0.0 { 1 } else { -1 }, cx);
            self.wheel_delta = 0.0;
        }
        self.wheel_reset = Some(cx.spawn(async move |this, cx| {
            Timer::after(Duration::from_millis(180)).await;
            let _ = this.update(cx, |this, _| this.wheel_delta = 0.0);
        }));
    }

    // ---- selection and reveal (Palette.tsx:727-732) ---------------------------

    fn select_prev(&mut self, _: &SelectPrev, _: &mut Window, cx: &mut Context<Self>) {
        emit(&self.log, json!({"event":"action","name":"SelectPrev"}));
        self.selected = self.selected.saturating_sub(1);
        self.reveal_nearest();
        cx.notify();
    }

    fn select_next(&mut self, _: &SelectNext, _: &mut Window, cx: &mut Context<Self>) {
        emit(&self.log, json!({"event":"action","name":"SelectNext"}));
        self.selected = (self.selected + 1).min(self.rows.len() - 1);
        self.reveal_nearest();
        cx.notify();
    }

    /// `scrollIntoView({block: "nearest"})`: no scroll when the row is fully visible.
    fn reveal_nearest(&self) {
        let viewport = self.list.viewport_bounds();
        if let Some(b) = self.list.bounds_for_item(self.selected)
            && b.top() >= viewport.top()
            && b.bottom() <= viewport.bottom()
        {
            return;
        }
        self.list.scroll_to_reveal_item(self.selected);
    }

    // ---- pointer drags (Palette.tsx:204-668) ----------------------------------

    fn row_at(&self, p: Point<Pixels>) -> Option<usize> {
        let viewport = self.list.viewport_bounds();
        if !viewport.contains(&p) {
            return None;
        }
        self.row_bounds.borrow().iter().find(|(_, b)| b.contains(&p)).map(|(ix, _)| *ix)
    }

    fn pointer_down(&mut self, ix: usize, e: &MouseDownEvent, cx: &mut Context<Self>) {
        if e.button != MouseButton::Left {
            return;
        }
        let row = &self.rows[ix];
        let kind = match row.kind {
            Kind::Header => DragKind::Category,
            Kind::App => DragKind::Reorder,
            Kind::File => DragKind::File,
        };
        self.drag = Some(PointerDrag {
            kind,
            source_id: row.id.clone(),
            start: e.position,
            pos: e.position,
            active: false,
            target_id: None,
        });
        self.raw_moves = 0;
        self.applies = 0;
        cx.notify();
    }

    /// One apply per frame, like the original's requestAnimationFrame coalescing.
    fn pointer_move(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        if self.drag.is_none() {
            return;
        }
        self.raw_moves += 1;
        self.pending_point = Some(p);
        if self.frame_scheduled {
            return;
        }
        self.frame_scheduled = true;
        let weak = cx.weak_entity();
        window.on_next_frame(move |window, cx| {
            let _ = weak.update(cx, |this, cx| {
                this.frame_scheduled = false;
                if let Some(p) = this.pending_point.take() {
                    this.apply(p, window, cx);
                }
            });
        });
        // request_animation_frame() asserts paint/prepaint in gpui 0.2.2; notify draws the frame.
        cx.notify();
    }

    fn apply(&mut self, p: Point<Pixels>, window: &mut Window, cx: &mut Context<Self>) {
        let target = self.row_at(p).map(|ix| self.rows[ix].clone());
        let Some(drag) = self.drag.as_mut() else { return };
        self.applies += 1;
        let dist = (f32::from(p.x - drag.start.x).powi(2) + f32::from(p.y - drag.start.y).powi(2)).sqrt();
        let crossed = dist >= DRAG_THRESHOLD;
        drag.pos = p;
        match drag.kind {
            DragKind::File => {
                if !drag.active && crossed {
                    drag.active = true;
                    self.start_ole(window, cx);
                }
            }
            DragKind::Reorder | DragKind::Category => {
                drag.active = drag.active || crossed;
                drag.target_id = if !drag.active {
                    None
                } else {
                    target.and_then(|t| {
                        let same_group = |a: &str| a.split(':').next().map(str::to_owned);
                        let ok = match drag.kind {
                            DragKind::Category => t.kind == Kind::Header,
                            _ => t.kind != Kind::Header && same_group(&t.id) == same_group(&drag.source_id),
                        };
                        (ok && t.id != drag.source_id).then_some(t.id)
                    })
                };
            }
        }
        cx.notify();
    }

    fn pointer_up(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if let Some(p) = self.pending_point.take() {
            self.apply(p, window, cx);
        }
        let Some(drag) = self.drag.take() else { return };
        emit(&self.log, json!({"event":"drag-finish","kind":format!("{:?}", drag.kind),"active":drag.active,
            "source":drag.source_id,"target":drag.target_id,"raw_moves":self.raw_moves,"applies":self.applies}));
        if drag.active
            && let Some(target) = &drag.target_id
        {
            let from = self.rows.iter().position(|r| r.id == drag.source_id);
            let to = self.rows.iter().position(|r| &r.id == target);
            if let (Some(from), Some(to)) = (from, to) {
                let row = self.rows.remove(from);
                self.rows.insert(to, row);
                emit(&self.log, json!({"event":"reorder","from":from,"to":to}));
            }
        }
        // The click that ends an active drag is not an activation.
        self.suppress_click = drag.active;
        if drag.active && drag.kind == DragKind::Reorder {
            self.leaving = Some(drag);
            self.leaving_task = Some(cx.spawn(async move |this, cx| {
                Timer::after(Duration::from_millis(PREVIEW_LEAVE_MS)).await;
                let _ = this.update(cx, |this, cx| {
                    this.leaving = None;
                    cx.notify();
                });
            }));
        }
        cx.notify();
    }

    fn cancel_drag(&mut self, cx: &mut Context<Self>) {
        self.pending_point = None;
        if let Some(drag) = self.drag.take() {
            emit(&self.log, json!({"event":"drag-cancel","kind":format!("{:?}", drag.kind)}));
        }
        cx.notify();
    }

    // ---- OLE file drag (drag.rs, lib.rs:1330) ---------------------------------

    fn start_ole(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let path = self.file_path.clone();
        let frames = self.frames.clone();
        match self.ole_mode {
            OleMode::Deferred => {
                self.ole_task = Some(cx.spawn_in(window, async move |this, cx| {
                    // No App borrow is held across this call: the modal loop may
                    // dispatch GPUI messages re-entrantly.
                    let f0 = frames.get();
                    let t0 = Instant::now();
                    let result = win::drag_file(&path);
                    let ms = t0.elapsed().as_millis();
                    let frames_during = frames.get() - f0;
                    let _ = this.update(cx, |this, cx| {
                        emit(&this.log, json!({"event":"ole-done","mode":"deferred","result":format!("{result:?}"),
                            "ms":ms,"frames_during_drag":frames_during}));
                        this.ole_result = Some(format!("{result:?}"));
                        this.drag = None;
                        cx.notify();
                    });
                }));
            }
            OleMode::Inline => {
                let f0 = frames.get();
                let result = win::drag_file(&path);
                emit(&self.log, json!({"event":"ole-done","mode":"inline","result":format!("{result:?}"),
                    "frames_during_drag":frames.get() - f0}));
                self.ole_result = Some(format!("{result:?}"));
                self.drag = None;
            }
        }
    }

    // ---- render ---------------------------------------------------------------

    fn render_row(&mut self, ix: usize, _: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let row = &self.rows[ix];
        let selected = ix == self.selected;
        let dragging = self.drag.as_ref().is_some_and(|d| d.active && d.source_id == row.id);
        let target = self.drag.as_ref().and_then(|d| d.target_id.as_ref()) == Some(&row.id);
        let bounds_sink = self.row_bounds.clone();
        let (height, text) = match row.kind {
            Kind::Header => (rems(1.75), rems(0.75)),
            _ => (rems(2.75), rems(0.84375)),
        };
        div()
            .id(("row", ix))
            .relative()
            .w_full()
            .h(height)
            .px(rems(0.75))
            .flex()
            .items_center()
            .text_size(text)
            .when(selected, |d| d.bg(rgba(0xffffff1f)))
            .when(dragging, |d| d.opacity(0.4))
            .when(target, |d| d.border_t_2().border_color(rgb(0x7aa2ff)))
            .child(format!("{}  [{}]", row.title, ix))
            .on_mouse_down(MouseButton::Left, cx.listener(move |this, e: &MouseDownEvent, _, cx| this.pointer_down(ix, e, cx)))
            .on_click(cx.listener(move |this, _, _, cx| {
                if std::mem::take(&mut this.suppress_click) {
                    emit(&this.log, json!({"event":"click-suppressed","ix":ix}));
                    return;
                }
                this.clicks.push(ix);
                emit(&this.log, json!({"event":"click","ix":ix}));
                cx.notify();
            }))
            .child(
                canvas(move |bounds, _, _| bounds_sink.borrow_mut().push((ix, bounds)), |_, _, _, _| {})
                    .absolute()
                    .size_full(),
            )
            .into_any_element()
    }

    /// ReorderDragPreview / CategoryDragPreview: offset from the pointer, clamped 8 px inside the window.
    fn preview_geometry(drag: &PointerDrag, vp: gpui::Size<Pixels>) -> (Pixels, Pixels, Pixels, Pixels) {
        let (w, h, x) = match drag.kind {
            DragKind::Category => (px(208.), px(48.), drag.pos.x + px(14.)),
            _ => (px(232.), px(54.), drag.pos.x - px(232.) - px(14.)),
        };
        let x = x.max(px(8.)).min(vp.width - w - px(8.));
        let y = (drag.pos.y - h / 2.).max(px(8.)).min(vp.height - h - px(8.));
        (x, y, w, h)
    }

    fn preview(&self, window: &Window) -> Option<AnyElement> {
        let (drag, leaving) = match (&self.drag, &self.leaving) {
            (Some(d), _) if d.active && d.kind != DragKind::File => (d, false),
            (_, Some(d)) => (d, true),
            _ => return None,
        };
        let (x, y, w, h) = Self::preview_geometry(drag, window.viewport_size());
        Some(
            div()
                .absolute()
                .left(x)
                .top(y)
                .w(w)
                .h(h)
                .rounded(px(10.))
                .bg(rgb(0x2a2a33))
                .border_1()
                .border_color(rgb(0x7aa2ff))
                .when(leaving, |d| d.opacity(0.5))
                .child(drag.source_id.clone())
                .into_any_element(),
        )
    }
}

impl Render for Spike {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        // A3: view zoom through the rem size only.
        window.set_rem_size(px(16.) * self.zoom as f32 / 100.);
        self.row_bounds.borrow_mut().clear();
        self.frames.set(self.frames.get() + 1);

        let weak = cx.weak_entity();
        let listeners = canvas(
            |_, _, _| (),
            move |_, _, window, _| {
                // Window-level listeners: moves outside the hovered element and
                // outside the window (GPUI holds SetCapture while a button is down).
                let w = weak.clone();
                window.on_mouse_event(move |e: &MouseMoveEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture {
                        let _ = w.update(cx, |this, cx| this.pointer_move(e.position, window, cx));
                    }
                });
                let w = weak.clone();
                window.on_mouse_event(move |e: &MouseUpEvent, phase, window, cx| {
                    if phase == DispatchPhase::Capture && e.button == MouseButton::Left {
                        let _ = w.update(cx, |this, cx| this.pointer_up(window, cx));
                    }
                });
                let w = weak.clone();
                window.on_mouse_event(move |e: &ScrollWheelEvent, phase, _, cx| {
                    if phase == DispatchPhase::Capture && e.modifiers.control {
                        cx.stop_propagation();
                        let _ = w.update(cx, |this, cx| this.zoom_wheel(e, cx));
                    }
                });
            },
        )
        .absolute()
        .size_full();

        let status = format!(
            "zoom {}%  sel {}  topmost? see log  ole {:?}",
            self.zoom, self.selected, self.ole_result
        );
        div()
            .id("root")
            .key_context("SpikeList")
            .track_focus(&self.focus)
            .on_action(cx.listener(Self::select_prev))
            .on_action(cx.listener(Self::select_next))
            .capture_key_down(cx.listener(|this, e: &KeyDownEvent, _, cx| {
                emit(&this.log, json!({"event":"capture_key_down","key":e.keystroke.key}));
                if e.keystroke.key == "escape" && this.drag.is_some() {
                    this.cancel_drag(cx);
                }
            }))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .rounded(px(14.))
            .bg(rgba(0x16161cf2))
            .text_color(rgb(0xf2f2f5))
            .font_family("Segoe UI Variable Text")
            .child(div().h(rems(3.)).px(rems(1.)).flex().items_center().child(status))
            .child(
                div()
                    .flex_1()
                    .child(list(self.list.clone(), cx.processor(Self::render_row)).size_full()),
            )
            .when(self.manual, |d| d.child(self.manual_panel()))
            .child(listeners)
            .children(self.preview(window))
    }
}

impl Spike {
    /// Manual mode: what to try, the live reduced-motion state, the last events.
    fn manual_panel(&self) -> impl IntoElement {
        let steps = [
            "1. Drag a FILES row out of this window and release over Explorer (a copy, a link, or nothing must happen).",
            "2. Drag a FILES row again and press Esc. Then drag one and release over this window.",
            "3. After each drag, click a row. A 'click' event must appear.",
            "4. Toggle Settings > Accessibility > Visual effects > Animation effects. Watch the line below.",
            "5. Ctrl+wheel, Ctrl+=/-/0, and drag APPS rows and headers. Close with Alt+F4.",
        ];
        let reduced = !win::client_area_animation();
        let log = self.log.borrow();
        let tail: Vec<String> = log.iter().rev().take(6).map(|v| v.to_string()).collect();
        div()
            .p(rems(0.75))
            .flex()
            .flex_col()
            .gap(rems(0.25))
            .text_size(rems(0.75))
            .bg(rgba(0x000000aa))
            .children(steps.iter().map(|t| div().child(*t)))
            .child(div().text_color(rgb(0x7aa2ff)).child(format!("reduced motion (SPI_GETCLIENTAREAANIMATION off): {reduced}")))
            .children(tail.into_iter().rev().map(|t| div().text_color(rgb(0x9a9aa6)).child(t)))
    }
}

// ---- scripted run -----------------------------------------------------------
//
// Off-screen, never activated, never topmost after its probe. Input is posted to
// this window only, so nothing reaches the foreground app. The OLE drag needs
// the real mouse button (DoDragDrop polls it), so it is a manual check.

struct Ctx {
    hwnd: windows::Win32::Foundation::HWND,
    scale: f32,
}

impl Ctx {
    /// Logical window point -> client point in physical pixels.
    fn client(&self, p: Point<Pixels>) -> (i32, i32) {
        ((f32::from(p.x) * self.scale).round() as i32, (f32::from(p.y) * self.scale).round() as i32)
    }
}

fn same(a: &ListOffset, b: &ListOffset) -> bool {
    a.item_ix == b.item_ix && a.offset_in_item == b.offset_in_item
}

async fn pause(ms: u64) {
    Timer::after(Duration::from_millis(ms)).await;
}

fn row_center(spike: &Spike, ix: usize) -> Option<Point<Pixels>> {
    spike.list.bounds_for_item(ix).map(|b| b.center())
}

async fn script(this: gpui::WeakEntity<Spike>, cx: &mut gpui::AsyncWindowContext, ctx: Ctx) -> anyhow::Result<()> {
    let log = this.update(cx, |s, _| s.log.clone())?;
    let out = |v: Value| emit(&log, v);
    pause(300).await;
    let frames0 = this.update(cx, |s, _| s.frames.get())?;
    out(json!({"probe":"env","window_rect":win::window_rect(ctx.hwnd),"is_foreground":win::foreground() == ctx.hwnd,
        "scale":ctx.scale,"wheel_scroll_lines":win::wheel_scroll_lines(),"frames_rendered":frames0}));

    // -- always on top (inventory row 92). SWP_NOACTIVATE / SW_SHOWNOACTIVATE only.
    let t0 = Instant::now();
    let before = win::is_topmost(ctx.hwnd);
    let on = win::set_topmost(ctx.hwnd, true);
    let after_on = win::is_topmost(ctx.hwnd);
    win::hide(ctx.hwnd);
    let hidden = !win::is_visible(ctx.hwnd);
    win::show_no_activate(ctx.hwnd);
    let topmost_after_hide_show = win::is_topmost(ctx.hwnd);
    let off = win::set_topmost(ctx.hwnd, false);
    let after_off = win::is_topmost(ctx.hwnd);
    win::send_to_back(ctx.hwnd);
    out(json!({"probe":"always-on-top","popup_kind_is_tool_window":win::is_tool_window(ctx.hwnd),
        "topmost_at_open":before,"set_on":format!("{on:?}"),"after_on":after_on,"hidden":hidden,
        "topmost_after_hide_show":topmost_after_hide_show,"set_off":format!("{off:?}"),"after_off":after_off,
        "us":t0.elapsed().as_micros()}));
    pause(150).await;

    // -- reduced motion (inventory row 12)
    let t0 = Instant::now();
    let animation = win::client_area_animation();
    out(json!({"probe":"reduced-motion","spi_client_area_animation":animation,"reduced_motion":!animation,
        "read_cost_us":t0.elapsed().as_micros()}));

    // -- zoom keys (inventory row 9)
    let chords: &[(&str, bool, bool, bool, VIRTUAL_KEY)] = &[
        ("Ctrl+Up", true, false, false, VK_UP),
        ("Ctrl+Down", true, false, false, VK_DOWN),
        ("Ctrl+=", true, false, false, VK_OEM_PLUS),
        ("Ctrl+Shift+= (+)", true, true, false, VK_OEM_PLUS),
        ("Ctrl+-", true, false, false, VK_OEM_MINUS),
        ("Ctrl+Shift+- (_)", true, true, false, VK_OEM_MINUS),
        ("Ctrl+NumpadAdd", true, false, false, VK_ADD),
        ("Ctrl+NumpadSubtract", true, false, false, VK_SUBTRACT),
        ("Ctrl+Numpad0", true, false, false, VK_NUMPAD0),
        ("Ctrl+0", true, false, false, VK_0),
        ("Ctrl+Shift+0 ())", true, true, false, VK_0),
        ("Ctrl+Alt+Up", true, false, true, VK_UP),
        ("Up", false, false, false, VK_UP),
        ("Down", false, false, false, VK_DOWN),
    ];
    for (name, ctrl, shift, alt, key) in chords {
        let (z0, s0, n0) = this.update(cx, |s, _| (s.zoom, s.selected, s.log.borrow().len()))?;
        win::set_mods(*ctrl, *shift, *alt);
        win::post_key(ctx.hwnd, *key, *alt);
        pause(80).await;
        win::set_mods(false, false, false);
        let (z1, s1, events) = this.update(cx, |s, _| {
            (s.zoom, s.selected, s.log.borrow()[n0..].iter().filter(|v| v.get("event").is_some()).cloned().collect::<Vec<_>>())
        })?;
        out(json!({"probe":"zoom-key","chord":name,"zoom_before":z0,"zoom_after":z1,
            "selection_moved":s0 != s1,"events":events}));
    }
    this.update(cx, |s, cx| { s.zoom = 100; s.selected = 1; cx.notify(); })?;
    pause(100).await;

    // -- Ctrl+wheel and plain wheel (inventory row 9)
    let center = this.update(cx, |s, _| s.list.viewport_bounds().center())?;
    let (cx_, cy_) = ctx.client(center);
    let (sx, sy) = win::client_to_screen(ctx.hwnd, cx_, cy_);
    let wheel_cases: [(&str, bool, i16, u64, u32); 6] = [
        ("ctrl, 1 notch up", true, 120, 250, 1),
        ("ctrl, 1 notch down", true, -120, 250, 1),
        ("ctrl, 3 x 1/3 notch up, 50 ms apart (precision wheel)", true, 40, 50, 3),
        ("ctrl, 1/3 notch up after a 250 ms gap", true, 40, 250, 1),
        ("ctrl, 1/3 notch down right after (sign change resets)", true, -40, 250, 1),
        ("plain, 1 notch down", false, -120, 250, 1),
    ];
    for (name, ctrl, delta, gap, reps) in wheel_cases {
        let (z0, top0) = this.update(cx, |s, _| (s.zoom, s.list.logical_scroll_top()))?;
        for _ in 0..reps {
            win::set_mods(ctrl, false, false);
            win::post_wheel(ctx.hwnd, sx, sy, delta, ctrl);
            pause(gap).await;
            win::set_mods(false, false, false);
        }
        let (z1, top1, acc) = this.update(cx, |s, _| (s.zoom, s.list.logical_scroll_top(), s.wheel_delta))?;
        out(json!({"probe":"wheel","case":name,"zoom_before":z0,"zoom_after":z1,"accumulator_after":acc,
            "list_scrolled": !same(&top0, &top1), "scroll_top_after": format!("{top1:?}")}));
    }
    // A3 check: row height follows the rem size.
    this.update(cx, |s, cx| { s.zoom = 100; s.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) }); cx.notify(); })?;
    pause(100).await;
    let h100 = this.update(cx, |s, _| s.list.bounds_for_item(1).map(|b| f32::from(b.size.height)))?;
    this.update(cx, |s, cx| { s.zoom = 150; cx.notify(); })?;
    pause(150).await;
    let h150 = this.update(cx, |s, _| s.list.bounds_for_item(1).map(|b| f32::from(b.size.height)))?;
    // An off-screen row: does ListState re-measure it after the rem size changed?
    let offscreen_150 = this.update(cx, |s, _| s.list.bounds_for_item(150).map(|b| f32::from(b.size.height)))?;
    this.update(cx, |s, cx| { s.list.reset(s.rows.len()); cx.notify(); })?;
    pause(150).await;
    let offscreen_150_after_reset = this.update(cx, |s, _| s.list.bounds_for_item(150).map(|b| f32::from(b.size.height)))?;
    this.update(cx, |s, cx| { s.zoom = 100; s.list.reset(s.rows.len()); cx.notify(); })?;
    pause(150).await;
    out(json!({"probe":"rem-zoom","row_h_100":h100,"row_h_150":h150,"offscreen_row_h_at_150":offscreen_150,
        "offscreen_row_h_at_150_after_reset":offscreen_150_after_reset}));

    // -- reveal (inventory row 116)
    let reveal = this.update(cx, |s, _| {
        let vp = s.list.viewport_bounds();
        let mut found = None;
        // A scroll offset that leaves a fully visible row less than
        // `offset_in_item` above the bottom edge (the reveal-item edge case).
        'outer: for k in 2..40 {
            for o in 1..40 {
                s.list.scroll_to(ListOffset { item_ix: k, offset_in_item: px(o as f32) });
                let Some(bk) = s.list.bounds_for_item(k) else { continue };
                if px(o as f32) >= bk.size.height { continue; }
                for j in (k + 1)..(k + 30) {
                    let Some(b) = s.list.bounds_for_item(j) else { continue };
                    let gap = vp.bottom() - b.bottom();
                    if b.top() >= vp.top() && gap > px(0.) && gap < px(o as f32) {
                        found = Some((k, o, j, f32::from(gap)));
                        break 'outer;
                    }
                }
            }
        }
        let Some((k, o, j, gap)) = found else { return json!({"probe":"reveal","error":"no edge case found"}) };
        let start = ListOffset { item_ix: k, offset_in_item: px(o as f32) };
        s.list.scroll_to(start);
        s.list.scroll_to_reveal_item(j);
        let raw_after = s.list.logical_scroll_top();
        s.list.scroll_to(start);
        s.selected = j;
        s.reveal_nearest();
        let nearest_after = s.list.logical_scroll_top();
        let below = j + 6;
        s.list.scroll_to(start);
        s.selected = below;
        s.reveal_nearest();
        let below_after = s.list.logical_scroll_top();
        let below_gap = s.list.bounds_for_item(below).map(|b| f32::from(vp.bottom() - b.bottom()));
        s.selected = k - 1;
        s.reveal_nearest();
        let above_after = s.list.logical_scroll_top();
        json!({"probe":"reveal","viewport_h":f32::from(vp.size.height),"start":format!("{start:?}"),
            "fully_visible_row":j,"gap_px":gap,
            "raw_scroll_to_reveal_item_after":format!("{raw_after:?}"),"raw_moved":!same(&raw_after, &start),
            "nearest_wrapper_after":format!("{nearest_after:?}"),"nearest_moved":!same(&nearest_after, &start),
            "below_row":below,"below_after":format!("{below_after:?}"),"below_bottom_gap_px":below_gap,
            "above_row":k - 1,"above_after":format!("{above_after:?}")})
    })?;
    out(reveal);
    this.update(cx, |s, cx| { s.selected = 1; s.list.scroll_to(ListOffset { item_ix: 0, offset_in_item: px(0.) }); cx.notify(); })?;
    pause(200).await;

    // -- pointer drags (inventory rows 19, 110, 111)
    async fn drag_path(cx: &mut gpui::AsyncWindowContext, this: &gpui::WeakEntity<Spike>, ctx: &Ctx,
        from: usize, to_ix: usize, escape: bool) -> anyhow::Result<Value> {
        let (a, b) = this.update(cx, |s, _| (row_center(s, from), row_center(s, to_ix)))?;
        let (Some(a), Some(b)) = (a, b) else { return Ok(json!({"error":"row not rendered"})) };
        let (ax, ay) = ctx.client(a);
        win::post_mouse_move(ctx.hwnd, ax, ay, false);
        win::post_mouse_down(ctx.hwnd, ax, ay);
        pause(40).await;
        // 3 px: below the 4 px threshold.
        win::post_mouse_move(ctx.hwnd, ax, ay + 3, true);
        pause(60).await;
        let active_at_3px = this.update(cx, |s, _| s.drag.as_ref().map(|d| d.active))?;
        // Ten moves posted at once: the frame coalescing must apply fewer times.
        let (bx, by) = ctx.client(b);
        for i in 1..=10 {
            win::post_mouse_move(ctx.hwnd, ax + i, ay + (by - ay) * i / 10, true);
        }
        pause(120).await;
        let mid = this.update_in(cx, |s, w, _| (s.drag.clone(), preview_rect(s, w), s.raw_moves, s.applies))?;
        // Far outside the window: the drag keeps tracking, the preview stays clamped.
        win::post_mouse_move(ctx.hwnd, -300, -300, true);
        pause(80).await;
        let outside = this.update_in(cx, |s, w, _| (s.drag.as_ref().map(|d| (f32::from(d.pos.x), f32::from(d.pos.y))), preview_rect(s, w)))?;
        win::post_mouse_move(ctx.hwnd, bx, by, true);
        pause(80).await;
        if escape {
            win::post_key(ctx.hwnd, VK_ESCAPE, false);
            pause(60).await;
        }
        win::post_mouse_up(ctx.hwnd, bx, by);
        pause(40).await;
        let leaving_after_40ms = this.update(cx, |s, _| s.leaving.is_some())?;
        pause(150).await;
        let leaving_after_190ms = this.update(cx, |s, _| s.leaving.is_some())?;
        let (clicks, order) = this.update(cx, |s, _| (s.clicks.clone(), s.rows.iter().take(12).map(|r| r.id.clone()).collect::<Vec<_>>()))?;
        Ok(json!({"from":from,"to":to_ix,"active_at_3px":active_at_3px,
            "mid_drag":mid.0.as_ref().map(|d| json!({"active":d.active,"target":d.target_id})),
            "preview_rect_xywh":mid.1,"raw_moves":mid.2,"coalesced_applies":mid.3,
            "outside_window":{"pointer":outside.0,"preview_rect_xywh":outside.1},
            "preview_leaving_after_40ms":leaving_after_40ms,"preview_leaving_after_190ms":leaving_after_190ms,
            "clicks_so_far":clicks,"order_head":order}))
    }

    let r = drag_path(cx, &this, &ctx, 7, 10, false).await?; // apps:1 -> apps:4
    out(json!({"probe":"drag-reorder","result":r}));
    let r = drag_path(cx, &this, &ctx, 7, 2, false).await?; // an apps row onto a files row: no target
    out(json!({"probe":"drag-reorder-other-group","result":r}));
    let r = drag_path(cx, &this, &ctx, 5, 0, false).await?; // "apps" header onto "files" header
    out(json!({"probe":"drag-category","result":r}));
    let r = drag_path(cx, &this, &ctx, 8, 11, true).await?; // Escape cancels
    out(json!({"probe":"drag-escape","result":r}));

    // A plain click after the drags must activate (no stuck suppression).
    let c = this.update(cx, |s, _| row_center(s, 12))?.unwrap();
    let (x, y) = ctx.client(c);
    win::post_mouse_move(ctx.hwnd, x, y, false);
    win::post_mouse_down(ctx.hwnd, x, y);
    win::post_mouse_up(ctx.hwnd, x, y);
    pause(120).await;
    out(json!({"probe":"click-after-drags","clicks":this.update(cx, |s, _| s.clicks.clone())?}));

    let frames1 = this.update(cx, |s, _| s.frames.get())?;
    out(json!({"probe":"done","frames_rendered":frames1,"is_foreground":win::foreground() == ctx.hwnd}));
    Ok(())
}

fn preview_rect(s: &Spike, w: &Window) -> Option<[f32; 4]> {
    let drag = match (&s.drag, &s.leaving) {
        (Some(d), _) if d.active && d.kind != DragKind::File => d,
        (_, Some(d)) => d,
        _ => return None,
    };
    let (x, y, pw, ph) = Spike::preview_geometry(drag, w.viewport_size());
    Some([f32::from(x), f32::from(y), f32::from(pw), f32::from(ph)])
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("chrome-probe") {
        chrome_probe::run();
        return;
    }
    let auto = args.get(1).map(String::as_str) == Some("auto");
    let ole_mode = if args.iter().any(|a| a == "--ole-inline") { OleMode::Inline } else { OleMode::Deferred };
    let file_path = std::env::current_dir().unwrap().join("tmp").join("drag-me.txt");
    std::fs::create_dir_all(file_path.parent().unwrap()).unwrap();
    std::fs::write(&file_path, "spike drag file\n").unwrap();

    Application::new().run(move |cx: &mut App| {
        cx.bind_keys([
            KeyBinding::new("up", SelectPrev, Some("SpikeList")),
            KeyBinding::new("down", SelectNext, Some("SpikeList")),
        ]);
        let log: Log = Rc::default();
        let window_size = size(px(720.), px(620.));
        // auto: off-screen and never activated, so the run is invisible.
        let bounds = if auto {
            Bounds::new(point(px(-4000.), px(-4000.)), window_size)
        } else {
            Bounds::centered(None, window_size, cx)
        };
        let handle = cx
            .open_window(
                WindowOptions {
                    window_bounds: Some(WindowBounds::Windowed(bounds)),
                    titlebar: None,
                    kind: WindowKind::PopUp,
                    is_resizable: false,
                    is_minimizable: false,
                    window_background: WindowBackgroundAppearance::Transparent,
                    focus: !auto,
                    show: true,
                    ..Default::default()
                },
                |window, cx| cx.new(|cx| Spike::new(window, cx, log.clone(), ole_mode, !auto, file_path.clone())),
            )
            .unwrap();
        if auto {
            handle
                .update(cx, |spike, window, cx| {
                    let ctx = Ctx { hwnd: win::hwnd_of(window), scale: window.scale_factor() };
                    win::send_to_back(ctx.hwnd);
                    let log = spike.log.clone();
                    cx.spawn_in(window, async move |this, cx| {
                        if let Err(e) = script(this, cx, ctx).await {
                            emit(&log, json!({"probe":"script-error","error":e.to_string()}));
                        }
                        let _ = cx.update(|_, cx| cx.quit());
                    })
                    .detach();
                })
                .unwrap();
        } else {
            cx.activate(true);
        }
    });
}
