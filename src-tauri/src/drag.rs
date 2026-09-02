//! Native Windows drag-and-drop support for dragging files (such as pictures)
//! out of Prism into external applications (Discord, Explorer, Photoshop, etc.).

use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use windows::core::{implement, BOOL, PCWSTR, HRESULT};
use windows::Win32::Foundation::{DRAGDROP_S_CANCEL, DRAGDROP_S_DROP, DRAGDROP_S_USEDEFAULTCURSORS, S_OK};
use windows::Win32::System::Com::IDataObject;
use windows::Win32::System::Ole::{
    DoDragDrop, OleInitialize, IDropSource, IDropSource_Impl,
    DROPEFFECT, DROPEFFECT_COPY, DROPEFFECT_LINK,
};
use windows::Win32::System::SystemServices::{MK_LBUTTON, MODIFIERKEYS_FLAGS};
use windows::Win32::UI::Shell::{
    BHID_DataObject, ILCreateFromPathW, ILFree, IShellItem,
    SHCreateItemFromParsingName, SHCreateShellItemArrayFromIDLists, Common::ITEMIDLIST,
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
}
