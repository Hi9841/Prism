//! Taskbar pins: read the pin state, and pin or unpin through the Explorer
//! bridge, the shell's pinned list, or the user.

use std::ffi::c_void;
use std::path::{Path, PathBuf};
use std::time::Duration;

use windows::core::{GUID, PCWSTR};
use windows::Win32::System::Com::{
    CoCreateInstance, CLSCTX_INPROC_SERVER,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{
    FOLDERID_RoamingAppData,
    ILCreateFromPathW, ILFree, SHParseDisplayName,
};
use crate::win32::{wide, ComGuard};
use crate::apps::{read_lnk_aumid, resolve_lnk};

/// CLSID_TaskbandPin and IID_IPinnedList3: undocumented COM interface used by
/// the Windows Shell (and browsers like Edge/Firefox) to pin and unpin items on the Taskbar.
const CLSID_TASKBAND_PIN: GUID = GUID::from_u128(0x90aa3a4e_1cba_4233_b8bb_535773d48449);
const IID_IPINNED_LIST3: GUID = GUID::from_u128(0x0dd79ae2_d156_45d4_9eeb_3b549769e940);
const PLMC_INT_MAX: i32 = 0x7fff_ffff;

#[repr(C)]
#[allow(non_snake_case)]
struct IPinnedList3Vtbl {
    pub QueryInterface: unsafe extern "system" fn(
        this: *mut c_void,
        riid: *const GUID,
        ppv: *mut *mut c_void,
    ) -> windows::core::HRESULT,
    pub AddRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub Release: unsafe extern "system" fn(this: *mut c_void) -> u32,
    /// Slots 3-8: EnumObjects, GetPinnableInfo, IsPinnable, Resolve,
    /// LegacyModify, GetChangeCount.
    pub BeforeIsPinned: [usize; 6],
    /// Slot 9: `S_OK` = pinned, `S_FALSE` = not pinned (matches Chromium's
    /// `taskbar_util.cc` IPinnedList3 layout).
    pub IsPinned: unsafe extern "system" fn(
        this: *mut c_void,
        pidl: *const ITEMIDLIST,
    ) -> windows::core::HRESULT,
    /// Slots 10-15: GetPinnedItem ... GetPinnedItemForAppID.
    pub BeforeModify: [usize; 6],
    pub Modify: unsafe extern "system" fn(
        this: *mut c_void,
        unpin: *const ITEMIDLIST,
        pin: *const ITEMIDLIST,
        caller: i32,
    ) -> windows::core::HRESULT,
}

#[repr(C)]
struct IPinnedList3 {
    pub vtbl: *const IPinnedList3Vtbl,
}

fn resolve_shell_pidl(path: &Path) -> Option<*mut ITEMIDLIST> {
    let wide_path = wide(&path.to_string_lossy());
    let mut pidl: *mut ITEMIDLIST = std::ptr::null_mut();
    let hr = unsafe { SHParseDisplayName(PCWSTR(wide_path.as_ptr()), None, &mut pidl, 0, None) };
    if hr.is_ok() && !pidl.is_null() {
        return Some(pidl);
    }
    let fallback = unsafe { ILCreateFromPathW(PCWSTR(wide_path.as_ptr())) };
    if !fallback.is_null() {
        Some(fallback)
    } else {
        None
    }
}

fn modify_taskband_pin(
    unpin_pidl: Option<*const ITEMIDLIST>,
    pin_pidl: Option<*const ITEMIDLIST>,
) -> Result<(), String> {
    let _com = ComGuard::init();
    unsafe {
        let unknown: windows::core::IUnknown =
            CoCreateInstance(&CLSID_TASKBAND_PIN, None, CLSCTX_INPROC_SERVER)
                .map_err(|error| format!("failed to instantiate TaskbandPin: {error}"))?;

        let mut pinned_list_raw: *mut c_void = std::ptr::null_mut();
        let hr = (windows::core::Interface::vtable(&unknown).QueryInterface)(
            windows::core::Interface::as_raw(&unknown),
            &IID_IPINNED_LIST3,
            &mut pinned_list_raw,
        );
        if hr.is_err() || pinned_list_raw.is_null() {
            return Err(format!("failed to query IPinnedList3: {hr:?}"));
        }

        let pinned_list = pinned_list_raw as *mut IPinnedList3;
        let mut modify_hr = ((*(*pinned_list).vtbl).Modify)(
            pinned_list_raw,
            unpin_pidl.unwrap_or(std::ptr::null()),
            pin_pidl.unwrap_or(std::ptr::null()),
            4, // PLMC_SHELL_PIN_UNPIN
        );
        if modify_hr.is_err() {
            modify_hr = ((*(*pinned_list).vtbl).Modify)(
                pinned_list_raw,
                unpin_pidl.unwrap_or(std::ptr::null()),
                pin_pidl.unwrap_or(std::ptr::null()),
                PLMC_INT_MAX,
            );
        }
        ((*(*pinned_list).vtbl).Release)(pinned_list_raw);

        modify_hr
            .ok()
            .map_err(|error| format!("taskbar pin modify failed: {error}"))
    }
}

const TASKBAR_STATE_CHECKS: usize = 10;
const TASKBAR_STATE_CHECK_INTERVAL: Duration = Duration::from_millis(75);

fn wait_for_taskbar_state_with<F>(
    pinned: bool,
    attempts: usize,
    delay: Duration,
    mut current_state: F,
) -> bool
where
    F: FnMut() -> bool,
{
    for attempt in 0..attempts {
        if current_state() == pinned {
            return true;
        }
        if attempt + 1 < attempts {
            std::thread::sleep(delay);
        }
    }
    false
}

fn wait_for_taskbar_state(path: &Path, pinned: bool) -> bool {
    wait_for_taskbar_state_with(
        pinned,
        TASKBAR_STATE_CHECKS,
        TASKBAR_STATE_CHECK_INTERVAL,
        || is_pinned_to_taskbar(path),
    )
}

/// Pins or unpins a launch target and returns success only after the taskbar
/// pin store reflects the requested state.
/// Sentinel returned when Windows refuses to pin programmatically. The
/// frontend turns this into the manual taskbar instruction.

pub fn set_taskbar_pinned(path: &Path, pinned: bool) -> Result<(), String> {
    if !path.exists() {
        return Err(format!("{} does not exist", path.display()));
    }
    // Unknown state must not be treated as "already in the desired state":
    // that reported a successful unpin without attempting one.
    let current = taskbar_pin_state(path);
    if current == Some(pinned) {
        return Ok(());
    }
    if current.is_none() {
        return Err("Could not read the current taskbar pin state.".to_string());
    }
    crate::logs::debug_trace(&format!(
        "set-pin begin path={} pinned={pinned}",
        path.display()
    ));

    let _com = ComGuard::init();
    // Unpin needs the shortcut that is actually in the taskbar store; pin uses
    // the original shortcut.
    let target = if pinned {
        path.to_path_buf()
    } else {
        find_taskbar_pin(path).unwrap_or_else(|| path.to_path_buf())
    };

    let mut result = crate::shell_bridge::taskbar_pin(&target, pinned);
    if result.is_ok() && wait_for_taskbar_state(path, pinned) {
        crate::logs::debug_trace("set-pin bridge ok");
        return Ok(());
    }

    if let Some(pidl) = resolve_shell_pidl(&target) {
        result = if pinned {
            modify_taskband_pin(None, Some(pidl))
        } else {
            modify_taskband_pin(Some(pidl), None)
        };
        unsafe { ILFree(Some(pidl)) };
        if result.is_ok() && wait_for_taskbar_state(path, pinned) {
            crate::logs::debug_trace("set-pin app-com ok");
            return Ok(());
        }
    }

    if pinned {
        // Windows 11 (24H2/25H2+) blocks programmatic pinning: Modify returns
        // S_OK but does nothing and no shell verb is exposed. Ask the user to
        // confirm from the taskbar instead of reporting a false success.
        crate::logs::debug_trace("set-pin manual required");
        Err(prism_vocab::MANUAL_PIN_REQUIRED.to_string())
    } else {
        Err("Windows did not unpin this item.".to_string())
    }
}

/// Finds the taskbar pin-store shortcut that represents a launch target, so
/// callers can check pin state or remove the exact pin.
pub fn find_taskbar_pin(path: &Path) -> Option<PathBuf> {
    let _com = ComGuard::init();
    let taskbar = taskbar_pins_dir()?;
    let Ok(entries) = std::fs::read_dir(&taskbar) else {
        return None;
    };
    let mut match_keys = vec![path_key(&path.to_string_lossy())];
    let path_aumid = read_lnk_aumid(path);
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
    {
        if let Some(resolved) = resolve_lnk(path) {
            if !resolved.target.is_empty() {
                match_keys.push(path_key(&resolved.target));
            }
        }
    }
    let requested_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| name.to_ascii_lowercase());
    for entry in entries.flatten() {
        let link = entry.path();
        if !link
            .extension()
            .is_some_and(|extension| extension.eq_ignore_ascii_case("lnk"))
        {
            continue;
        }
        if match_keys.contains(&path_key(&link.to_string_lossy())) {
            return Some(link);
        }
        if let (Some(ref req_aumid), Some(link_aumid)) = (&path_aumid, read_lnk_aumid(&link)) {
            if !req_aumid.is_empty() && req_aumid.eq_ignore_ascii_case(&link_aumid) {
                return Some(link);
            }
        }
        let Some(resolved) = resolve_lnk(&link) else {
            continue;
        };
        if resolved.target.is_empty() {
            let link_name = link
                .file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.to_ascii_lowercase());
            if requested_name.is_some() && link_name == requested_name {
                return Some(link);
            }
        } else if match_keys.contains(&path_key(&resolved.target)) {
            return Some(link);
        }
    }
    None
}

/// Reports whether a launch target already sits on the taskbar's pin list.
/// Uses `IPinnedList3::IsPinned`, which is authoritative. The on-disk pin
/// folder is NOT: it keeps stale shortcuts for items that are no longer pinned.
pub fn is_pinned_to_taskbar(path: &Path) -> bool {
    taskbar_pin_state(path).unwrap_or(false)
}

/// `Some(true)` = pinned, `Some(false)` = not pinned, `None` = unknown (the
/// shell rejected the query, e.g. no PIDL for the path).
fn taskbar_pin_state(path: &Path) -> Option<bool> {
    let _com = ComGuard::init();
    let unknown: windows::core::IUnknown =
        unsafe { CoCreateInstance(&CLSID_TASKBAND_PIN, None, CLSCTX_INPROC_SERVER) }.ok()?;
    let mut pinned_list_raw: *mut c_void = std::ptr::null_mut();
    let hr = unsafe {
        (windows::core::Interface::vtable(&unknown).QueryInterface)(
            windows::core::Interface::as_raw(&unknown),
            &IID_IPINNED_LIST3,
            &mut pinned_list_raw,
        )
    };
    if hr.is_err() || pinned_list_raw.is_null() {
        return None;
    }
    let pinned_list = pinned_list_raw as *mut IPinnedList3;
    let Some(pidl) = resolve_shell_pidl(path) else {
        unsafe { ((*(*pinned_list).vtbl).Release)(pinned_list_raw) };
        return None;
    };
    let state = unsafe { ((*(*pinned_list).vtbl).IsPinned)(pinned_list_raw, pidl) };
    unsafe {
        ILFree(Some(pidl));
        ((*(*pinned_list).vtbl).Release)(pinned_list_raw);
    }
    match state.0 {
        0 => Some(true),
        1 => Some(false),
        _ => None,
    }
}

fn taskbar_pins_dir() -> Option<PathBuf> {
    Some(
        crate::win32::known_folder(&FOLDERID_RoamingAppData)?
            .join("Microsoft")
            .join("Internet Explorer")
            .join("Quick Launch")
            .join("User Pinned")
            .join("TaskBar"),
    )
}

/// Case-insensitive comparison key for Windows filesystem paths coming from
/// different shell APIs: trims separators and normalizes slashes and casing.
fn path_key(path: &str) -> String {
    path.trim()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_ascii_lowercase()
}
