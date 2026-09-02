//! Native Windows drag-and-drop support for dragging files (such as pictures)
//! out of Prism into external applications (Discord, Explorer, Photoshop, etc.).

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{implement, BOOL, PCWSTR, HRESULT, Interface};
use windows::Win32::Foundation::{
    COLORREF, DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS,
    POINT, SIZE, S_OK,
};
use windows::Win32::Graphics::Gdi::{DeleteObject, GetObjectW, BITMAP, HBITMAP};
use windows::Win32::System::Com::{CoCreateInstance, CLSCTX_INPROC_SERVER, IDataObject};
use windows::Win32::System::Ole::{
    DoDragDrop, OleInitialize, IDropSource, IDropSource_Impl,
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILCreateFromPathW, ILFree, IShellItem, IShellItemImageFactory,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, Common::ITEMIDLIST,
    IDragSourceHelper, SHDRAGIMAGE, SIIGBF_BIGGERSIZEOK, SIIGBF_RESIZETOFIT,
};

#[implement(IDropSource)]
struct DropSource(());

#[allow(non_snake_case)]
impl IDropSource_Impl for DropSource_Impl {
    fn QueryContinueDrag(&self, fescapepressed: BOOL, grfkeystate: MODIFIERKEYS_FLAGS) -> HRESULT {
        if fescapepressed.as_bool() {
            DRAGDROP_S_CANCEL
        } else if (grfkeystate & MK_LBUTTON) == MODIFIERKEYS_FLAGS(0) {
            DRAGDROP_S_DROP
        } else {
            S_OK
        }
    }

    fn GiveFeedback(&self, _dweffect: DROPEFFECT) -> HRESULT {
        DRAGDROP_S_USEDEFAULTCURSORS
    }
}

pub static DRAG_IN_PROGRESS: AtomicBool = AtomicBool::new(false);

pub fn is_dragging() -> bool {
    DRAG_IN_PROGRESS.load(Ordering::Acquire)
}

pub fn init_thread_ole() {
    unsafe {
        let _ = OleInitialize(None);
    }
}

fn wide_null(path: &Path) -> Vec<u16> {
    path.as_os_str().encode_wide().chain(std::iter::once(0)).collect()
}

pub fn get_file_data_object(paths: &[PathBuf]) -> Result<IDataObject, String> {
    if paths.is_empty() {
        return Err("No file paths provided for drag".to_string());
    }

    for path in paths {
        if !path.exists() {
            return Err(format!("{} does not exist", path.display()));
        }
    }

    init_thread_ole();

    unsafe {
        if paths.len() == 1 {
            let wide = wide_null(&paths[0]);
            let item: IShellItem = SHCreateItemFromParsingName(PCWSTR(wide.as_ptr()), None)
                .map_err(|e| format!("Failed to create shell item for {}: {e}", paths[0].display()))?;
            let data_object: IDataObject = item.BindToHandler(None, &BHID_DataObject)
                .map_err(|e| format!("Failed to obtain IDataObject from shell item: {e}"))?;
            return Ok(data_object);
        }

        let mut pidls = Vec::with_capacity(paths.len());
        for path in paths {
            let wide = wide_null(path);
            let pidl = ILCreateFromPathW(PCWSTR(wide.as_ptr()));
            if pidl.is_null() {
                for p in &pidls {
                    ILFree(Some(*p));
                }
                return Err(format!("Failed to resolve shell ID list for {}", path.display()));
            }
            pidls.push(pidl);
        }

        let const_pidls: Vec<*const ITEMIDLIST> = pidls.iter().map(|&p| p as *const ITEMIDLIST).collect();
        let array_res = SHCreateShellItemArrayFromIDLists(&const_pidls);

        for pidl in pidls {
            ILFree(Some(pidl));
        }

        let array = array_res.map_err(|e| format!("Failed to create shell item array: {e}"))?;
        let data_object: IDataObject = array.BindToHandler(None, &BHID_DataObject)
            .map_err(|e| format!("Failed to obtain IDataObject from shell item array: {e}"))?;
        Ok(data_object)
    }
}

const CLSID_DRAG_DROP_HELPER: windows::core::GUID =
    windows::core::GUID::from_u128(0x4657278a_411b_11d2_839a_00c04fd918d0);

fn attach_drag_image(data_object: &IDataObject, paths: &[PathBuf]) {
    let Some(first_path) = paths.first() else { return; };
    let wide_path = wide_null(first_path);
    let shell_item: IShellItem = match unsafe {
        SHCreateItemFromParsingName(PCWSTR(wide_path.as_ptr()), None)
    } {
        Ok(item) => item,
        Err(_) => return,
    };

    let Ok(factory): Result<IShellItemImageFactory, _> = shell_item.cast() else { return; };

    let target_size = SIZE { cx: 96, cy: 96 };
    let hbitmap: HBITMAP = match unsafe {
        factory.GetImage(target_size, SIIGBF_BIGGERSIZEOK | SIIGBF_RESIZETOFIT)
    } {
        Ok(h) => h,
        Err(_) => return,
    };

    let mut bm = BITMAP::default();
    let bm_size = std::mem::size_of::<BITMAP>() as i32;
    let actual_size = if unsafe {
        GetObjectW(hbitmap.into(), bm_size, Some(&mut bm as *mut _ as *mut _))
    } == bm_size {
        SIZE { cx: bm.bmWidth, cy: bm.bmHeight }
    } else {
        target_size
    };

    let helper: IDragSourceHelper = match unsafe {
        CoCreateInstance(
            &CLSID_DRAG_DROP_HELPER,
            None,
            CLSCTX_INPROC_SERVER,
        )
    } {
        Ok(h) => h,
        Err(_) => {
            unsafe { let _ = DeleteObject(hbitmap.into()); }
            return;
        }
    };

    let offset = POINT {
        x: actual_size.cx / 2,
        y: actual_size.cy / 2,
    };

    let shdragimage = SHDRAGIMAGE {
        sizeDragImage: actual_size,
        ptOffset: offset,
        hbmpDragImage: hbitmap,
        crColorKey: COLORREF(0),
    };

    let result = unsafe {
        helper.InitializeFromBitmap(&shdragimage, Some(data_object))
    };
    if result.is_err() {
        unsafe { let _ = DeleteObject(hbitmap.into()); }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DragResult {
    Dropped,
    Cancelled,
}

/// Executes a native Windows OLE drag-and-drop operation for the given file paths.
/// Blocks until the user either drops the files on a target application or cancels.
pub fn drag_files(paths: &[PathBuf]) -> Result<DragResult, String> {
    DRAG_IN_PROGRESS.store(true, Ordering::Release);
    struct DragGuard;
    impl Drop for DragGuard {
        fn drop(&mut self) {
            DRAG_IN_PROGRESS.store(false, Ordering::Release);
        }
    }
    let _guard = DragGuard;

    init_thread_ole();
    let data_object = get_file_data_object(paths)?;
    attach_drag_image(&data_object, paths);
    let drop_source: IDropSource = DropSource(()).into();

    unsafe {
        let mut effect = DROPEFFECT::default();
        let hr = DoDragDrop(&data_object, &drop_source, DROPEFFECT_COPY | DROPEFFECT_LINK, &mut effect);
        if hr == DRAGDROP_S_DROP {
            Ok(DragResult::Dropped)
        } else {
            Ok(DragResult::Cancelled)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs::File;

    #[test]
    fn get_file_data_object_rejects_empty_paths() {
        let err = get_file_data_object(&[]).unwrap_err();
        assert!(err.contains("No file paths provided"));
    }

    #[test]
    fn get_file_data_object_rejects_nonexistent_paths() {
        let missing = PathBuf::from(r"C:\nonexistent_file_12345.png");
        let err = get_file_data_object(&[missing]).unwrap_err();
        assert!(err.contains("does not exist"));
    }

    #[test]
    fn get_file_data_object_succeeds_for_existing_file() {
        let temp_dir = std::env::temp_dir().join(format!("prism-drag-test-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("test_image.png");
        File::create(&file_path).unwrap();

        let obj = get_file_data_object(&[file_path]);
        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(obj.is_ok(), "Should obtain IDataObject for valid file");
    }

    #[test]
    fn get_file_data_object_succeeds_for_multiple_files() {
        let temp_dir = std::env::temp_dir().join(format!("prism-drag-test-multi-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let f1 = temp_dir.join("img1.png");
        let f2 = temp_dir.join("img2.jpg");
        File::create(&f1).unwrap();
        File::create(&f2).unwrap();

        let obj = get_file_data_object(&[f1, f2]);
        let _ = std::fs::remove_dir_all(&temp_dir);
        assert!(obj.is_ok(), "Should obtain IDataObject for multiple valid files");
    }

    #[test]
    fn drag_state_is_inactive_by_default() {
        assert!(!is_dragging());
    }

    #[test]
    fn attach_drag_image_runs_cleanly_for_valid_file() {
        let temp_dir = std::env::temp_dir().join(format!("prism-drag-img-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&temp_dir);
        let file_path = temp_dir.join("sample.png");
        File::create(&file_path).unwrap();

        let obj = get_file_data_object(&[file_path.clone()]).unwrap();
        attach_drag_image(&obj, &[file_path]);
        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
