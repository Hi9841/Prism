#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    prism_lib::attach_to_default_desktop();
    if prism_lib::run_start_restore_watchdog_if_requested() {
        return;
    }
    if prism_lib::run_taskbar_repair_if_requested() {
        return;
    }
    prism_lib::run();
}
