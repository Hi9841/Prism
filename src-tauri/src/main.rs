#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    if prism_lib::run_start_restore_watchdog_if_requested() {
        return;
    }
    prism_lib::run();
}
