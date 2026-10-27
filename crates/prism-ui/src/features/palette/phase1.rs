//! Palette items built from backend Phase 1 hits (`src/features/palette/phase1.ts`).

use std::collections::HashMap;

use serde_json::json;

use crate::lib::query::{Phase1Hit, PowerPlanHit};
use crate::lib::types::{AppEntry, LucideIcon, PaletteIcon, PaletteItem, RowAction, TileTint, is_elevatable_path};

/// `ICONS[key]`. `Home` is `House` in lucide.
fn icon_for_key(key: &str) -> Option<(LucideIcon, TileTint)> {
    Some(match key {
        "nightlight" => (LucideIcon("Moon"), "amber"),
        "display" => (LucideIcon("Monitor"), "azure"),
        "hdr" => (LucideIcon("Sun"), "amber"),
        "bluetooth" => (LucideIcon("Bluetooth"), "azure"),
        "sound" => (LucideIcon("Volume2"), "mint"),
        "storage" => (LucideIcon("HardDrive"), "slate"),
        "power" => (LucideIcon("Power"), "rose"),
        "update" => (LucideIcon("RefreshCw"), "azure"),
        "network" => (LucideIcon("Wifi"), "azure"),
        "mute" => (LucideIcon("VolumeX"), "rose"),
        "unmute" => (LucideIcon("Volume2"), "mint"),
        "lock" => (LucideIcon("Lock"), "slate"),
        "sleep" => (LucideIcon("Moon"), "iris"),
        "shutdown" => (LucideIcon("Power"), "rose"),
        "restart" => (LucideIcon("RotateCcw"), "amber"),
        "window" => (LucideIcon("AppWindow"), "azure"),
        "settings" => (LucideIcon("House"), "azure"),
        "control-panel" => (LucideIcon("Settings2"), "azure"),
        "device" => (LucideIcon("Cpu"), "azure"),
        "performance" => (LucideIcon("Wrench"), "amber"),
        "power-plan" => (LucideIcon("BatteryCharging"), "mint"),
        "tool" => (LucideIcon("Wrench"), "slate"),
        _ => return None,
    })
}

pub fn action_palette_item(hit: &Phase1Hit) -> PaletteItem {
    // BUG(port): an iconKey that names an Object.prototype member ("toString")
    // breaks the source; the port falls back to "settings".
    let (icon, tint) = icon_for_key(hit.icon_key.as_deref().unwrap_or("settings"))
        .unwrap_or_else(|| icon_for_key("settings").expect("settings icon"));
    let action_id = match &hit.action_id {
        Some(id) => id.clone(),
        None => hit.id.strip_prefix("action::").unwrap_or(&hit.id).to_owned(),
    };
    let icon = match hit.icon.as_deref().filter(|icon| !icon.is_empty()) {
        Some(src) => PaletteIcon::App { name: hit.title.clone(), icon: Some(src.to_owned()) },
        None => PaletteIcon::Tile { icon, tint },
    };
    let mut item = PaletteItem::new(
        hit.id.clone(),
        hit.title.clone(),
        icon,
        hit.title.clone(),
        RowAction::call("execute_action", json!({ "id": action_id })),
    );
    item.subtitle = hit.subtitle.clone();
    item.copy_path = hit.uri.clone().filter(|uri| !uri.is_empty());
    item
}

pub fn power_plan_palette_item(hit: &PowerPlanHit) -> PaletteItem {
    let mut item = PaletteItem::new(
        format!("power-plan::{}", hit.guid),
        hit.name.clone(),
        PaletteIcon::Tile { icon: LucideIcon("Power"), tint: if hit.active { "mint" } else { "slate" } },
        hit.name.clone(),
        RowAction::call("set_power_plan", json!({ "guid": hit.guid })),
    );
    item.subtitle = Some(if hit.active { "Power plan · Active now" } else { "Power plan" }.into());
    item.power_plan_guid = Some(hit.guid.clone());
    item
}

pub fn window_palette_item(hit: &Phase1Hit) -> PaletteItem {
    let (icon, tint) = icon_for_key("window").expect("window icon");
    let run = match hit.hwnd {
        None => RowAction::None,
        Some(hwnd) => RowAction::call("focus_window", json!({ "hwnd": hwnd })),
    };
    let mut item =
        PaletteItem::new(hit.id.clone(), hit.title.clone(), PaletteIcon::Tile { icon, tint }, hit.title.clone(), run);
    item.subtitle = hit.subtitle.clone();
    item
}

pub fn app_hit_palette_item(
    hit: &Phase1Hit,
    apps_by_id: &HashMap<&str, &AppEntry>,
    icons: &HashMap<String, String>,
    to_item: impl Fn(&AppEntry) -> PaletteItem,
) -> Option<PaletteItem> {
    let app_id = hit.app_id.as_deref().filter(|id| !id.is_empty())?;
    if let Some(entry) = apps_by_id.get(app_id) {
        return Some(to_item(entry));
    }
    let mut item = PaletteItem::new(
        hit.id.clone(),
        hit.title.clone(),
        PaletteIcon::App { name: hit.title.clone(), icon: icons.get(app_id).cloned() },
        hit.title.clone(),
        RowAction::call("launch_app", json!({ "id": app_id })),
    );
    item.subtitle = hit.subtitle.clone();
    item.app_id = Some(app_id.to_owned());
    item.run_as_admin = is_elevatable_path(hit.path.as_deref())
        .then(|| RowAction::call("launch_app_as_admin", json!({ "id": app_id })));
    item.copy_path = hit
        .path
        .clone()
        .filter(|path| !path.is_empty())
        .or_else(|| crate::features::palette::sections::apps_folder_path(Some(app_id)));
    Some(item)
}

pub fn is_command_action(hit: &Phase1Hit) -> bool {
    hit.kind == "action" && hit.uri.as_deref().is_none_or(str::is_empty)
}

// PORT STATUS: confidence=high todos=0
