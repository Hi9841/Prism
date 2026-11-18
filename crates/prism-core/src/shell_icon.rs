//! Icons from the shell as PNG bytes or data URLs: a file's icon, a shell
//! item's image, and the icon of an Apps folder entry.

use std::ffi::c_void;
use std::io::Cursor;

use windows::core::{Interface, PCWSTR};
use windows::Win32::Foundation::{HWND, SIZE};
use windows::Win32::Graphics::Gdi::{
    CreateCompatibleDC, DeleteDC, DeleteObject, GetDC, GetDIBits, GetObjectW, ReleaseDC,
    SelectObject, BITMAP, BITMAPINFO, BITMAPINFOHEADER, BI_RGB, DIB_RGB_COLORS, HBITMAP, HGDIOBJ,
};
use windows::Win32::UI::Shell::Common::ITEMIDLIST;
use windows::Win32::UI::Shell::{IShellFolder, IShellItem, IShellItemImageFactory, SIIGBF_ICONONLY};

use crate::win32::{wide, wide_trim};

/// Extracts an icon for a filesystem path (exe, ico, .lnk or .url) via
/// SHGetFileInfoW, which lets the shell resolve shortcut icons.
pub(crate) unsafe fn extract_file_icon(path: &str) -> Option<Vec<u8>> {
    use windows::Win32::Storage::FileSystem::FILE_FLAGS_AND_ATTRIBUTES;
    use windows::Win32::UI::Shell::{
        SHGetFileInfoW, SHFILEINFOW, SHGFI_FLAGS, SHGFI_ICON, SHGFI_LARGEICON,
    };
    use windows::Win32::UI::WindowsAndMessaging::DestroyIcon;

    let wide_path = wide(path);
    let mut info = std::mem::zeroed::<SHFILEINFOW>();
    let result = SHGetFileInfoW(
        PCWSTR(wide_path.as_ptr()),
        FILE_FLAGS_AND_ATTRIBUTES(0),
        Some(&mut info),
        std::mem::size_of::<SHFILEINFOW>() as u32,
        SHGFI_FLAGS(SHGFI_ICON.0 | SHGFI_LARGEICON.0),
    );
    if result == 0 || info.hIcon.0.is_null() {
        return None;
    }
    let png = hicon_to_png(info.hIcon);
    let _ = DestroyIcon(info.hIcon);
    png
}

/// Returns a PNG data URL for a shell item's native icon.
pub fn shell_item_icon_data_url(item: &IShellItem) -> Option<String> {
    unsafe { extract_shell_item_icon(item).map(|png| png_data_url(&png)) }
}

unsafe fn extract_shell_item_icon(item: &IShellItem) -> Option<Vec<u8>> {
    let factory = item.cast::<IShellItemImageFactory>().ok()?;
    let bitmap = factory
        .GetImage(SIZE { cx: 64, cy: 64 }, SIIGBF_ICONONLY)
        .ok()?;
    bitmap_to_png(bitmap)
}

/// Extracts an icon for a virtual AppsFolder item through the Shell image
/// factory. Modern AppX/MSIX apps expose their package logo this way rather
/// than as an icon resource filename. Older shell extensions fall back to
/// IExtractIconW below.
pub(crate) unsafe fn extract_apps_folder_icon(
    item: &IShellItem,
    folder: &IShellFolder,
    pidl: *mut ITEMIDLIST,
) -> Option<Vec<u8>> {
    extract_shell_item_icon(item).or_else(|| extract_legacy_apps_folder_icon(folder, pidl))
}

unsafe fn extract_legacy_apps_folder_icon(
    folder: &IShellFolder,
    pidl: *mut ITEMIDLIST,
) -> Option<Vec<u8>> {
    use windows::Win32::UI::Shell::{
        ExtractIconExW, IExtractIconW, GIL_FORSHELL, GIL_NOTFILENAME, GIL_OPENICON,
    };
    use windows::Win32::UI::WindowsAndMessaging::{DestroyIcon, HICON};

    let icon: IExtractIconW = folder
        .GetUIObjectOf(
            HWND(std::ptr::null_mut()),
            &[pidl as *const ITEMIDLIST],
            None,
        )
        .ok()?;
    let mut path_buf = [0u16; 260];
    let mut index = 0i32;
    let mut flags = 0u32;
    icon.GetIconLocation(
        GIL_OPENICON | GIL_FORSHELL,
        &mut path_buf,
        &mut index,
        &mut flags,
    )
    .ok()?;
    let path = wide_trim(&path_buf);
    let mut hicon = HICON(std::ptr::null_mut());
    // GIL_NOTFILENAME means pszIconFile is a resource spec, not a file.
    if flags & GIL_NOTFILENAME == 0 && !path.is_empty() {
        let wide_path = wide(&path);
        let count = ExtractIconExW(PCWSTR(wide_path.as_ptr()), index, Some(&mut hicon), None, 1);
        if count == 0 {
            hicon.0 = std::ptr::null_mut();
        }
    }
    if hicon.0.is_null() {
        let wide_path = wide(&path);
        icon.Extract(
            PCWSTR(wide_path.as_ptr()),
            index as u32,
            Some(&mut hicon),
            None,
            0,
        )
        .ok()?;
    }
    if hicon.0.is_null() {
        return None;
    }
    let png = hicon_to_png(hicon);
    let _ = DestroyIcon(hicon);
    png
}

/// Converts an HICON to PNG bytes: GetIconInfo gives the color bitmap,
/// which bitmap_to_png renders (BGRA -> RGBA with alpha handling).
unsafe fn hicon_to_png(icon: windows::Win32::UI::WindowsAndMessaging::HICON) -> Option<Vec<u8>> {
    use windows::Win32::Graphics::Gdi::HGDIOBJ;
    use windows::Win32::UI::WindowsAndMessaging::{GetIconInfo, ICONINFO};

    let mut info = std::mem::zeroed::<ICONINFO>();
    if GetIconInfo(icon, &mut info).is_err() {
        return None;
    }
    let color = info.hbmColor;
    let mask = info.hbmMask;
    let png = if !color.0.is_null() {
        bitmap_to_png(color)
    } else {
        // Monochrome icons carry no color bitmap; skip rather than render
        // a black square.
        let _ = DeleteObject(HGDIOBJ(mask.0));
        None
    };
    if !color.0.is_null() && !mask.0.is_null() {
        let _ = DeleteObject(HGDIOBJ(mask.0));
    }
    png
}

/// Converts an HBITMAP to PNG bytes (BGRA -> RGBA via GetDIBits, with
/// premultiplied-alpha and no-alpha handling). Always consumes the bitmap.
unsafe fn bitmap_to_png(bitmap: HBITMAP) -> Option<Vec<u8>> {
    let bitmap_handle = HGDIOBJ(bitmap.0);
    let mut bmp: BITMAP = std::mem::zeroed();
    let copied = GetObjectW(
        bitmap_handle,
        std::mem::size_of::<BITMAP>() as i32,
        Some(&mut bmp as *mut _ as *mut c_void),
    );
    if copied == 0 {
        let _ = DeleteObject(bitmap_handle);
        return None;
    }
    let width = bmp.bmWidth;
    let height = bmp.bmHeight;
    if width <= 0 || height <= 0 || width > 512 || height > 512 {
        let _ = DeleteObject(bitmap_handle);
        return None;
    }
    let screen = GetDC(None);
    let memory = CreateCompatibleDC(Some(screen));
    let old = SelectObject(memory, bitmap_handle);
    let mut info: BITMAPINFO = std::mem::zeroed();
    info.bmiHeader.biSize = std::mem::size_of::<BITMAPINFOHEADER>() as u32;
    info.bmiHeader.biWidth = width;
    info.bmiHeader.biHeight = -height; // top-down rows
    info.bmiHeader.biPlanes = 1;
    info.bmiHeader.biBitCount = 32;
    info.bmiHeader.biCompression = BI_RGB.0;
    let mut pixels = vec![0u8; width as usize * height as usize * 4];
    let lines = GetDIBits(
        memory,
        bitmap,
        0,
        height as u32,
        Some(pixels.as_mut_ptr() as *mut c_void),
        &mut info,
        DIB_RGB_COLORS,
    );
    let _ = SelectObject(memory, old);
    let _ = DeleteDC(memory);
    if !screen.is_invalid() {
        let _ = ReleaseDC(None, screen);
    }
    let _ = DeleteObject(bitmap_handle);
    if lines <= 0 {
        return None;
    }
    // GetDIBits yields BGRA; shell bitmaps arrive with premultiplied alpha.
    let has_alpha = pixels.as_chunks::<4>().0.iter().any(|p| p[3] != 0);
    if !has_alpha {
        // GetDIBits writes BGRA. No alpha channel: swap to RGBA and force opaque.
        for px in pixels.as_chunks_mut::<4>().0 {
            px.swap(0, 2);
            px[3] = 255;
        }
    } else {
        for px in pixels.as_chunks_mut::<4>().0 {
            let (b, g, r, a) = (px[0] as u32, px[1] as u32, px[2] as u32, px[3] as u32);
            let scale = |c: u32| ((c * 255) / a.max(1)).min(255) as u8;
            px[0] = scale(r);
            px[1] = scale(g);
            px[2] = scale(b);
        }
    }
    let image = image::RgbaImage::from_raw(width as u32, height as u32, pixels)?;
    let mut out = Cursor::new(Vec::new());
    image.write_to(&mut out, image::ImageFormat::Png).ok()?;
    Some(out.into_inner())
}

pub(crate) fn png_data_url(bytes: &[u8]) -> String {
    use base64::Engine;
    format!("data:image/png;base64,{}", base64::engine::general_purpose::STANDARD.encode(bytes))
}
