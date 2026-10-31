//! The footer update button (`src/features/updater/UpdateControl.tsx`).

use std::sync::{Arc, Mutex};
use std::time::Duration;

use gpui::{Context, Entity, ParentElement, Render, Styled, Task, Timer, Window, div, prelude::*, relative};

use crate::components::ui::{text_width, theme, title_tooltip, truncate, weight};
use crate::features::updater::update_policy::should_check_for_update;
use crate::features::updater::update_progress::update_percent;
use crate::features::updater::updater::{self, DownloadEvent, Update};
use crate::gfx::{Tween, icon, motion_ms, r};
use crate::lib::bridge::{self, BackendEvent};
use crate::state::app::{AppState, ToastKind, app_state, error_string};
use crate::theme::{TRANSPARENT, hsla, motion};
use crate::transition::{self, Timing};

const CHECK_INTERVAL_MS: u64 = 60 * 60 * 1000;
const NETWORK_TIMEOUT: Duration = Duration::from_secs(10);
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Debug, PartialEq)]
enum ViewState {
    Hidden,
    Available { version: String },
    Saving { version: String },
    Downloading { version: String, downloaded: u64, total: Option<u64> },
    Installing { version: String },
    Failed { version: String },
}

pub struct UpdateControl {
    app: Entity<AppState>,
    view: ViewState,
    update: Option<Update>,
    check_in_flight: Option<Task<()>>,
    install_in_flight: bool,
    install_generation: u64,
    last_check_at: f64,
    force_check_pending: bool,
    current_version: String,
    _interval: Task<()>,
    _subscriptions: Vec<gpui::Subscription>,
}

fn now_ms() -> f64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_millis() as f64).unwrap_or(0.0)
}

impl UpdateControl {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let hub = bridge::event_hub(cx);
        let sub = cx.subscribe(&hub, |this, _, event: &BackendEvent, cx| {
            if matches!(event, BackendEvent::Toggle { open: true }) {
                this.check_for_update(true, cx);
            }
        });
        let interval = cx.spawn(async move |this, cx| {
            loop {
                Timer::after(Duration::from_millis(CHECK_INTERVAL_MS)).await;
                if this.update(cx, |this, cx| this.check_for_update(false, cx)).is_err() {
                    return;
                }
            }
        });
        let mut this = Self {
            app: app_state(cx),
            view: ViewState::Hidden,
            update: None,
            check_in_flight: None,
            install_in_flight: false,
            install_generation: 0,
            last_check_at: 0.0,
            force_check_pending: false,
            current_version: bridge::get_app_version(cx),
            _interval: interval,
            _subscriptions: vec![sub],
        };
        this.check_for_update(false, cx);
        this
    }

    fn check_for_update(&mut self, force: bool, cx: &mut Context<Self>) {
        if self.install_in_flight {
            return;
        }
        if self.check_in_flight.is_some() {
            if force {
                self.force_check_pending = true;
            }
            return;
        }
        let now = now_ms();
        // Opens are explicit requests for current release information, but the
        // policy floor keeps Win-key spam from firing repeated no-cache requests.
        if !should_check_for_update(self.last_check_at, now, force) {
            return;
        }
        self.last_check_at = now;
        let generation = self.install_generation;
        let version = self.current_version.clone();
        self.check_in_flight = Some(cx.spawn(async move |this, cx| {
            let result = cx
                .background_spawn(async move { updater::check(NETWORK_TIMEOUT, force, &version) })
                .await;
            let _ = this.update(cx, |this, cx| {
                if generation == this.install_generation {
                    match result {
                        Ok(None) => {
                            this.update = None;
                            this.view = ViewState::Hidden;
                        }
                        Ok(Some(update)) => {
                            this.view = ViewState::Available { version: update.version.clone() };
                            this.update = Some(update);
                        }
                        Err(error) => {
                            prism_core::win_key::debug_trace(&format!("update-check-failed {error}"));
                            this.view = ViewState::Failed { version: "latest".into() };
                        }
                    }
                }
                this.check_in_flight = None;
                if this.force_check_pending && !this.install_in_flight {
                    this.force_check_pending = false;
                    this.check_for_update(true, cx);
                }
                cx.notify();
            });
        }));
    }

    fn install_update(&mut self, cx: &mut Context<Self>) {
        if self.install_in_flight {
            return;
        }
        let Some(update) = self.update.clone() else {
            if matches!(self.view, ViewState::Failed { .. }) {
                self.check_for_update(true, cx);
            }
            return;
        };
        if matches!(self.view, ViewState::Downloading { .. } | ViewState::Installing { .. }) {
            return;
        }
        self.install_in_flight = true;
        self.install_generation += 1;
        let version = update.version.clone();
        self.view = ViewState::Saving { version: version.clone() };
        cx.notify();
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let mut saving = true;
            let result: Result<(), String> = async {
                let flush = app.update(cx, |app, cx| app.flush_persistence(cx)).map_err(|e| e.to_string())?;
                flush.await?;
                saving = false;
                let progress = Arc::new(Mutex::new((0u64, None::<u64>)));
                let _ = this.update(cx, |this, cx| {
                    this.view = ViewState::Downloading { version: version.clone(), downloaded: 0, total: None };
                    cx.notify();
                });
                let sink = progress.clone();
                let download_update = update.clone();
                let download = cx.background_spawn(async move {
                    updater::download(&download_update, DOWNLOAD_TIMEOUT, |event| {
                        let mut state = sink.lock().unwrap_or_else(|e| e.into_inner());
                        match event {
                            DownloadEvent::Started { content_length } => state.1 = content_length,
                            DownloadEvent::Progress { chunk_length } => state.0 += chunk_length,
                        }
                    })
                });
                // Mirror the progress into the view while the download runs.
                let poll_version = version.clone();
                let poll_progress = progress.clone();
                let poll = this.update(cx, |_, cx| {
                    cx.spawn(async move |this, cx| loop {
                        Timer::after(Duration::from_millis(100)).await;
                        let (downloaded, total) = *poll_progress.lock().unwrap_or_else(|e| e.into_inner());
                        let alive = this.update(cx, |this, cx| {
                            if matches!(this.view, ViewState::Downloading { .. }) {
                                this.view =
                                    ViewState::Downloading { version: poll_version.clone(), downloaded, total };
                                cx.notify();
                            }
                        });
                        if alive.is_err() {
                            return;
                        }
                    })
                });
                let bytes = download.await;
                drop(poll);
                let bytes = bytes?;
                // Settings can change during a long download. Flush again
                // immediately before handing control to the installer.
                saving = true;
                let _ = this.update(cx, |this, cx| {
                    this.view = ViewState::Saving { version: version.clone() };
                    cx.notify();
                });
                let flush = app.update(cx, |app, cx| app.flush_persistence(cx)).map_err(|e| e.to_string())?;
                flush.await?;
                saving = false;
                let _ = this.update(cx, |this, cx| {
                    this.view = ViewState::Installing { version: version.clone() };
                    cx.notify();
                });
                updater::install(&update, &bytes)
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                this.install_in_flight = false;
                if let Err(error) = result {
                    this.view = ViewState::Failed { version: version.clone() };
                    let title = if saving { "Settings not saved" } else { "Update failed" };
                    app.update(cx, |app, cx| app.show_toast(title, Some(error_string(&error)), ToastKind::Error, cx));
                }
                cx.notify();
            });
        })
        .detach();
    }
}

/// One icon in the cross-fading stack (`.icon-swap`, 300 ms).
fn swap_icon(
    id: &str,
    name: &str,
    visible: bool,
    color: crate::theme::Rgba,
    spin: bool,
    window: &mut Window,
    cx: &mut gpui::App,
) -> gpui::AnyElement {
    let state = window.use_keyed_state(gpui::ElementId::Name(id.to_owned().into()), cx, |_, _| {
        Tween::at(if visible { 1.0 } else { 0.0 })
    });
    let target = if visible { 1.0 } else { 0.0 };
    if state.read(cx).target() != target {
        state.update(cx, |s, _| s.set(target, motion_ms(300), motion::icon_swap));
    }
    let p = state.read(cx).animate(window);
    let scale = 0.25 + 0.75 * p;
    let transform = if spin {
        // `animate-spin`: one turn per second, linear.
        window.request_animation_frame();
        let turn = (std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_millis()
            % 1000) as f32
            / 1000.0;
        gpui::Transformation::rotate(gpui::radians(turn * std::f32::consts::TAU)).with_scaling(gpui::size(scale, scale))
    } else {
        gpui::Transformation::scale(gpui::size(scale, scale))
    };
    let glyph = icon(name, 14., color).with_transformation(transform);
    div().absolute().inset_0().flex().items_center().justify_center().opacity(p).child(glyph).into_any_element()
}

impl Render for UpdateControl {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, _zoom) = theme(cx);
        if self.view == ViewState::Hidden {
            // `.press`: color, background-color, and scale.
            let key = gpui::ElementId::Name("update-check".into());
            let paint = transition::hover_paint(&key, (t.fg_quiet, t.fg), (TRANSPARENT, t.surface_hover), Timing::PRESS, window, cx);
            let button = transition::press(div().id(key.clone()), &key, true, window, cx);
            return crate::components::focus::button(button, true, &t)
                .tooltip(title_tooltip("Check for updates"))
                .flex()
                .flex_none()
                .items_center()
                .justify_center()
                .size(r(32.))
                .rounded(r(7.))
                .bg(hsla(paint.bg))
                .on_click(cx.listener(|this, _, _, cx| this.check_for_update(true, cx)))
                .child(icon("refresh-cw", 14., paint.text))
                .child(paint.tracker)
                .into_any_element();
        }
        let version = match &self.view {
            ViewState::Available { version }
            | ViewState::Saving { version }
            | ViewState::Downloading { version, .. }
            | ViewState::Installing { version }
            | ViewState::Failed { version } => version.trim_start_matches(['v', 'V']).to_owned(),
            ViewState::Hidden => String::new(),
        };
        let busy = matches!(self.view, ViewState::Saving { .. } | ViewState::Downloading { .. } | ViewState::Installing { .. });
        let failed = matches!(self.view, ViewState::Failed { .. });
        let percent = match &self.view {
            ViewState::Downloading { downloaded, total, .. } => {
                update_percent(*downloaded as f64, total.map(|t| t as f64))
            }
            _ => None,
        };
        let label = match &self.view {
            ViewState::Available { .. } => format!("Update v{version}"),
            ViewState::Saving { .. } => "Saving settings".into(),
            ViewState::Downloading { .. } => match percent {
                Some(p) => format!("Update {}%", crate::js::number_to_string(p)),
                None => "Downloading".into(),
            },
            ViewState::Installing { .. } => "Installing".into(),
            _ => "Retry update".into(),
        };
        let _ = text_width;
        let title = if failed {
            format!("Retry Prism v{version} update")
        } else if busy {
            format!("{label} Prism v{version}")
        } else {
            format!("Install Prism v{version}")
        };
        let (bg, fg, hover_bg, hover_fg) = if failed {
            (t.danger_soft, t.danger, t.danger_soft, t.danger)
        } else if busy {
            (t.surface, t.fg_secondary, t.surface, t.fg_secondary)
        } else {
            (t.accent_soft, t.accent, t.surface_active, t.fg)
        };
        // `.press`: color, background-color, opacity, and scale; the icons follow the text color.
        let key = gpui::ElementId::Name("update-install".into());
        let paint = transition::hover_paint(&key, (fg, hover_fg), (bg, hover_bg), Timing::PRESS, window, cx);
        let opacity = transition::number(&key, "opacity", if failed && paint.hovered { 0.9 } else { 1.0 }, Timing::PRESS, window, cx);
        let icon_color = paint.text;
        let button = transition::press(div().id(key.clone()), &key, !busy, window, cx);
        crate::components::focus::button(button, !busy, &t)
            .tooltip(title_tooltip(title))
            .flex()
            .flex_none()
            .items_center()
            .justify_center()
            .gap(r(6.))
            .h(r(32.))
            .w(r(112.))
            .min_w_0()
            .px(r(10.))
            .rounded(r(7.))
            .text_size(r(11.))
            .font_weight(weight(600.))
            .line_height(relative(1.45))
            .bg(hsla(paint.bg))
            .text_color(hsla(paint.text))
            .opacity(opacity)
            .when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.install_update(cx))))
            .child(
                div()
                    .relative()
                    .flex_none()
                    .size(r(14.))
                    .child(swap_icon("update-busy", "loader-circle", busy, icon_color, busy, window, cx))
                    .child(swap_icon("update-failed", "circle-alert", failed, icon_color, false, window, cx))
                    .child(swap_icon("update-ready", "arrow-down-to-line", !busy && !failed, icon_color, false, window, cx)),
            )
            .child(crate::theme::tabular_nums(truncate(label)))
            .child(paint.tracker)
            .into_any_element()
    }
}

// PORT STATUS: confidence=medium todos=0
