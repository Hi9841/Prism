//! Taskbar settings (`src/components/TaskbarCustomization.tsx`).

use std::future::Future;
use std::sync::Arc;

use base64::Engine;
use gpui::{
    AnyElement, App, Context, Entity, Image, ParentElement, PathPromptOptions, Render, SharedString, Styled, Window, div,
    img, prelude::*, relative,
};

use crate::components::ui::{SegmentOption, segmented, settings_row, shadow_sm, theme, title_tooltip, toggle, weight};
use crate::gfx::{icon, png_image, r, zoomed};
use crate::lib::bridge::{self, TaskbarSettings};
use crate::state::app::{AppState, ToastKind, app_state, error_string};
use crate::theme::{Rgba, TRANSPARENT, Theme, fade, hsla};
use crate::transition::{self, Timing};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum BusyControl {
    Load,
    Thickness,
    AutoHide,
    Combine,
    Icon,
}

pub struct TaskbarCustomization {
    app: Entity<AppState>,
    settings: Option<TaskbarSettings>,
    busy: Option<BusyControl>,
    custom_images: Vec<(String, Arc<Image>)>,
}

impl TaskbarCustomization {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let mut this = Self { app: app_state(cx), settings: None, busy: Some(BusyControl::Load), custom_images: Vec::new() };
        this.refresh(cx);
        this
    }

    fn set_settings(&mut self, settings: TaskbarSettings) {
        self.custom_images = settings
            .custom_start_icons
            .iter()
            .filter_map(|icon| {
                let bytes = base64::engine::general_purpose::STANDARD.decode(&icon.preview).ok()?;
                Some((icon.id.clone(), png_image(bytes)))
            })
            .collect();
        self.settings = Some(settings);
    }

    fn refresh(&mut self, cx: &mut Context<Self>) {
        self.busy = Some(BusyControl::Load);
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = bridge::get_taskbar_settings().await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(settings) => this.set_settings(settings),
                    Err(error) => app.update(cx, |app, cx| {
                        app.show_toast("Taskbar settings unavailable", Some(error_string(&error)), ToastKind::Success, cx)
                    }),
                }
                this.busy = None;
                cx.notify();
            });
        })
        .detach();
    }

    fn apply<F>(&mut self, control: BusyControl, action: F, cx: &mut Context<Self>)
    where
        F: Future<Output = Result<(), String>> + 'static,
    {
        if self.busy.is_some() {
            return;
        }
        self.busy = Some(control);
        cx.notify();
        let app = self.app.clone();
        cx.spawn(async move |this, cx| {
            let result = async {
                action.await?;
                bridge::get_taskbar_settings().await
            }
            .await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok(settings) => this.set_settings(settings),
                    Err(error) => app.update(cx, |app, cx| {
                        app.show_toast("Taskbar not changed", Some(error_string(&error)), ToastKind::Success, cx)
                    }),
                }
                this.busy = None;
                cx.notify();
            });
        })
        .detach();
    }

    /// The `<input type="file" multiple accept=".png">` flow: the native picker
    /// runs from a foreground task (A7), then each PNG is sent to the backend.
    fn choose_icon(&mut self, cx: &mut Context<Self>) {
        if self.busy.is_some() {
            return;
        }
        let paths = cx.prompt_for_paths(PathPromptOptions {
            files: true,
            directories: false,
            multiple: true,
            prompt: None,
        });
        cx.spawn(async move |this, cx| {
            let Ok(Ok(Some(paths))) = paths.await else { return };
            if paths.is_empty() {
                return;
            }
            let _ = this.update(cx, |this, cx| {
                this.apply(
                    BusyControl::Icon,
                    async move {
                        for path in paths {
                            let bytes = std::fs::read(&path).map_err(|error| error.to_string())?;
                            if bytes.len() > 2 * 1024 * 1024 {
                                return Err("Choose PNGs smaller than 2 MB".into());
                            }
                            let is_png = path.extension().is_some_and(|e| e.eq_ignore_ascii_case("png"))
                                || bytes.starts_with(b"\x89PNG");
                            if !is_png {
                                return Err("Choose PNG images".into());
                            }
                            bridge::set_custom_start_icon(bytes).await?;
                        }
                        Ok(())
                    },
                    cx,
                )
            });
        })
        .detach();
    }
}

/// A Start icon tile and its text color (for a glyph that follows it).
/// Colors move like `.press`; `:hover` also matches a disabled button.
fn icon_tile(
    id: impl Into<gpui::ElementId>,
    selected: bool,
    disabled: bool,
    t: &Theme,
    window: &mut Window,
    cx: &mut App,
) -> (gpui::Stateful<gpui::Div>, Rgba) {
    let id: gpui::ElementId = id.into();
    let (paint, border) = if selected {
        let fill = fade(t.accent, 12.0);
        (transition::hover_paint(&id, (t.fg, t.fg), (fill, fill), Timing::PRESS, window, cx), fade(t.accent, 70.0))
    } else {
        (transition::hover_paint(&id, (t.fg_tertiary, t.fg), (t.surface, t.surface_hover), Timing::PRESS, window, cx), TRANSPARENT)
    };
    let border = transition::color(&id, "border", border, Timing::PRESS, window, cx);
    // The `<fieldset disabled>` makes every tile `:disabled` while busy.
    let tile = transition::press(div().id(id.clone()), &id, !disabled, window, cx);
    let tile = crate::components::focus::button(tile, !disabled, t)
        .flex()
        .flex_col()
        .h(r(48.))
        .min_w(r(46.))
        .items_center()
        // `grid` with auto rows: content taller than the box overflows at the bottom, and
        // the rows do not shrink: the 24px icon box gets `flex_shrink_0` (the label cannot
        // shrink below its line).
        .justify_start()
        .gap(r(2.))
        .rounded(r(8.))
        .border_1()
        .px(r(8.))
        .py(r(4.))
        .text_size(r(10.))
        .font_weight(weight(500.))
        .line_height(relative(1.45))
        .border_color(hsla(border))
        .bg(hsla(paint.bg))
        .text_color(hsla(paint.text))
        .child(paint.tracker);
    (tile, paint.text)
}

fn windows_glyph(t: &Theme) -> AnyElement {
    let square = || div().bg(hsla(t.fg_tertiary));
    div()
        .grid()
        .grid_cols(2)
        .size(r(16.))
        .gap(r(1.5))
        .child(square())
        .child(square())
        .child(square())
        .child(square())
        .into_any_element()
}

impl Render for TaskbarCustomization {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (t, zoom) = theme(cx);
        let Some(settings) = self.settings.clone() else {
            return div()
                .flex()
                .min_h(r(48.))
                .items_center()
                .justify_center()
                .child(crate::features::palette::palette::spinning_icon("loader-circle", 16., t.fg_tertiary))
                .into_any_element();
        };
        let busy = self.busy.is_some();
        let this = cx.entity().downgrade();
        let density = segmented(
            "taskbar-density",
            &settings.thickness,
            vec![
                SegmentOption { value: "compact".into(), label: "Compact".into() },
                SegmentOption { value: "default".into(), label: "Default".into() },
                SegmentOption { value: "adaptive".into(), label: "When full".into() },
            ],
            busy,
            &t,
            zoom,
            window,
            cx,
            {
                let this = this.clone();
                move |value: SharedString, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.apply(BusyControl::Thickness, bridge::set_taskbar_thickness(value.to_string()), cx)
                    });
                }
            },
        );
        let auto_hide = toggle("taskbar-auto-hide", settings.auto_hide, busy, &t, zoom, window, cx, {
            let this = this.clone();
            move |value, _, cx| {
                let _ = this.update(cx, |this, cx| this.apply(BusyControl::AutoHide, bridge::set_taskbar_auto_hide(value), cx));
            }
        });
        let combine = segmented(
            "taskbar-combine",
            &settings.combine_buttons,
            vec![
                SegmentOption { value: "always".into(), label: "Always".into() },
                SegmentOption { value: "whenFull".into(), label: "When full".into() },
                SegmentOption { value: "never".into(), label: "Never".into() },
            ],
            busy,
            &t,
            zoom,
            window,
            cx,
            {
                let this = this.clone();
                move |value: SharedString, _, cx| {
                    let _ = this.update(cx, |this, cx| {
                        this.apply(BusyControl::Combine, bridge::set_taskbar_combine_buttons(value.to_string()), cx)
                    });
                }
            },
        );

        let options: [(&str, &str); 3] = [("system", "System"), ("gem", "Gem"), ("diamond", "Diamond")];
        let mut tiles = div().flex().max_w(r(260.)).flex_wrap().justify_end().gap(r(6.));
        for (index, (value, label)) in options.iter().enumerate() {
            let selected = settings.start_icon == *value;
            let value = value.to_string();
            let glyph: AnyElement = match index {
                0 => windows_glyph(&t),
                // Chromium rasterizes an `<img>` SVG at its painted size. GPUI's `img` rasterizes
                // it at its 96px intrinsic size and samples that down with no anti-aliasing, so
                // the one-color icons draw as masks in their stroke color.
                _ => {
                    let stroke = if value == "gem" { 0xf8fafc } else { 0x60a5fa };
                    gpui::svg()
                        .path(SharedString::from(format!("taskbar-icons/{value}.svg")))
                        .size(r(20.))
                        .flex_none()
                        .text_color(hsla(gpui::rgb(stroke)))
                        .into_any_element()
                }
            };
            let (tile, _) = icon_tile(("start-icon", index), selected, busy, &t, window, cx);
            tiles = tiles.child(
                tile
                    .tooltip(title_tooltip(*label))
                    .when(!busy, |d| {
                        d.on_click(cx.listener(move |this, _, _, cx| {
                            this.apply(BusyControl::Icon, bridge::set_taskbar_start_icon(value.clone()), cx)
                        }))
                    })
                    .child(div().flex().flex_shrink_0().size(r(24.)).items_center().justify_center().child(glyph))
                    .child(*label),
            );
        }
        let (tile, glyph) = icon_tile("start-icon-custom", false, busy, &t, window, cx);
        tiles = tiles.child(
            tile
                .tooltip(title_tooltip("Add custom PNG"))
                .when(!busy, |d| d.on_click(cx.listener(|this, _, _, cx| this.choose_icon(cx))))
                .child(div().flex().flex_shrink_0().size(r(24.)).items_center().justify_center().child(icon("image-up", 16., glyph)))
                .child("Custom"),
        );
        for (index, (id, image)) in self.custom_images.clone().into_iter().enumerate() {
            let selected = settings.start_icon == "custom" && settings.selected_custom_icon.as_deref() == Some(id.as_str());
            let select_id = id.clone();
            let remove_id = id.clone();
            // React keys custom icons by id.
            let (tile, _) = icon_tile(gpui::ElementId::Name(format!("custom-icon-{id}").into()), selected, busy, &t, window, cx);
            let remove_key = gpui::ElementId::Name(format!("custom-icon-remove-{id}").into());
            let remove = transition::hover_paint(&remove_key, (t.fg_tertiary, t.danger), (t.bg_raised, t.bg_raised), Timing::PRESS, window, cx);
            let remove_button = transition::press(div().id(remove_key.clone()), &remove_key, !busy, window, cx);
            tiles = tiles.child(
                div()
                    .relative()
                    .h(r(48.))
                    .min_w(r(46.))
                    .child(
                        tile
                            .tooltip(title_tooltip(format!("Custom icon {}", index + 1)))
                            .when(!busy, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply(BusyControl::Icon, bridge::select_custom_start_icon(select_id.clone()), cx)
                                }))
                            })
                            .child(div().flex().flex_shrink_0().size(r(24.)).items_center().justify_center().child(img(image).size(r(20.))))
                            .child(format!("Custom {}", index + 1)),
                    )
                    .child(
                        // `shadow-sm` (utilities layer) overrides the `.focus-ring` box-shadow
                        // (components layer): this badge shows no focus ring.
                        // Occlude: the badge sits on the tile, and a press on it must not
                        // also press, hover, or click the tile (CSS hits the top element only).
                        remove_button
                            .occlude()
                            .tab_index(0)
                            .tab_stop(!busy)
                            .tooltip(title_tooltip(format!("Remove custom icon {}", index + 1)))
                            .absolute()
                            .top(r(-4.))
                            .right(r(-4.))
                            .flex()
                            .items_center()
                            .justify_center()
                            .size(r(20.))
                            .rounded_full()
                            .border_1()
                            .border_color(hsla(t.line))
                            .bg(hsla(t.bg_raised))
                            .shadow(zoomed(shadow_sm(), zoom))
                            .when(!busy, |d| {
                                d.on_click(cx.listener(move |this, _, _, cx| {
                                    this.apply(BusyControl::Icon, bridge::remove_custom_start_icon(remove_id.clone()), cx)
                                }))
                            })
                            .child(icon("x", 12., remove.text))
                            .child(remove.tracker),
                    ),
            );
        }

        div()
            .child(settings_row(
                "Taskbar density",
                Some("Controls icon scale; Windows 11 keeps the taskbar surface height fixed"),
                density,
                &t,
            ))
            .child(settings_row("Auto-hide taskbar", Some("Removes the taskbar until you move to its edge"), auto_hide, &t))
            .child(settings_row("Combine buttons", Some("Controls labels and task grouping"), combine, &t))
            .child(settings_row(
                "Start button icon",
                Some("Recommended: 96 x 96 PNG with a transparent background"),
                tiles,
                &t,
            ))
            .into_any_element()
    }
}

// PORT STATUS: confidence=medium todos=0
