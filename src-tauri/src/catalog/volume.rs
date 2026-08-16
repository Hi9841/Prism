use std::collections::HashSet;
use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::path::{Path, PathBuf};

use windows::core::PCWSTR;
use windows::Win32::Storage::FileSystem::{
    FindFirstVolumeW, FindNextVolumeW, FindVolumeClose, GetDriveTypeW, GetLogicalDriveStringsW,
    GetVolumeInformationW, GetVolumeNameForVolumeMountPointW, GetVolumePathNamesForVolumeNameW,
};
use windows::Win32::System::WindowsProgramming::{
    DRIVE_CDROM, DRIVE_FIXED, DRIVE_RAMDISK, DRIVE_REMOTE, DRIVE_REMOVABLE,
};

use super::types::VolumeInfo;

const GUID_PREFIX: &str = r"\\?\Volume{";

pub fn discover_volumes() -> Vec<VolumeInfo> {
    let mut records = Vec::new();

    for volume_guid_path in enumerate_volume_guids() {
        if let Some(volume) = inspect_guid_volume(&volume_guid_path, "volume-guid", None) {
            records.push(volume);
        }
    }

    // Logical drives are a safety net for mounts omitted by GUID enumeration
    // and for mapped or unusual drives that cannot resolve to a volume GUID.
    for root in get_logical_drives() {
        match volume_name_for_mount_point(&root) {
            Ok(volume_guid_path) => {
                if let Some(volume) =
                    inspect_guid_volume(&volume_guid_path, "logical-drive", Some(root.clone()))
                {
                    records.push(volume);
                }
            }
            Err(error) => {
                debug_api_error(
                    "GetVolumeNameForVolumeMountPointW",
                    &root.to_string_lossy(),
                    &error,
                );
                if let Some(volume) = inspect_logical_fallback(&root) {
                    records.push(volume);
                }
            }
        }
    }

    let volumes = merge_volume_records(records);
    if super::catalog_debug_enabled() {
        for volume in &volumes {
            eprintln!(
                "{}",
                serde_json::json!({
                    "event": "volume_discovered",
                    "sources": volume.discovery_sources,
                    "raw_volume_guid_path": volume.volume_guid_path,
                    "volume_id": volume.volume_id,
                    "mount_paths": volume.mount_paths.iter().map(|p| p.to_string_lossy()).collect::<Vec<_>>(),
                    "canonical_mount_path": volume.canonical_mount_path().map(|p| p.to_string_lossy().into_owned()),
                    "drive_letter": volume.drive_letter,
                    "drive_type": volume.drive_type,
                    "file_system": volume.fs_type,
                    "label": volume.label,
                    "accessible": volume.accessible,
                })
            );
        }
    }
    volumes
}

impl VolumeInfo {
    pub fn normalized_mount_paths(&self) -> Vec<PathBuf> {
        normalize_mount_paths(&self.mount_paths)
    }

    pub fn canonical_mount_path(&self) -> Option<PathBuf> {
        canonical_mount_path_with(&self.mount_paths, |path| path.try_exists().unwrap_or(false))
    }

    pub fn recompute_mount_metadata(&mut self) {
        self.mount_paths = self.normalized_mount_paths();
        self.drive_letter = self
            .canonical_mount_path()
            .as_deref()
            .and_then(drive_letter_from_mount_path)
            .or_else(|| {
                self.mount_paths
                    .iter()
                    .find_map(|path| drive_letter_from_mount_path(path))
            });
        self.accessible = self
            .canonical_mount_path()
            .is_some_and(|path| path.try_exists().unwrap_or(false));
    }
}

pub fn drive_letter_from_mount_path(path: &Path) -> Option<String> {
    let value = path.as_os_str().to_string_lossy();
    let bytes = value.as_bytes();
    if bytes.len() == 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        Some(format!("{}:", (bytes[0] as char).to_ascii_uppercase()))
    } else {
        None
    }
}

pub(crate) fn canonical_mount_path_with(
    paths: &[PathBuf],
    accessible: impl Fn(&Path) -> bool,
) -> Option<PathBuf> {
    let mut paths = normalize_mount_paths(paths);
    paths.sort_by(|left, right| {
        let left_accessible = accessible(left);
        let right_accessible = accessible(right);
        let left_drive = drive_letter_from_mount_path(left).is_some();
        let right_drive = drive_letter_from_mount_path(right).is_some();
        (
            !left_accessible,
            !left_drive,
            component_count(left),
            mount_key(left),
        )
            .cmp(&(
                !right_accessible,
                !right_drive,
                component_count(right),
                mount_key(right),
            ))
    });
    paths.into_iter().next()
}

pub(crate) fn merge_volume_records(records: Vec<VolumeInfo>) -> Vec<VolumeInfo> {
    let mut merged: Vec<VolumeInfo> = Vec::new();
    for mut incoming in records {
        incoming.recompute_mount_metadata();
        let position = merged
            .iter()
            .position(|existing| records_represent_same_volume(existing, &incoming));
        if let Some(position) = position {
            merge_into(&mut merged[position], incoming);
        } else {
            merged.push(incoming);
        }
    }
    for volume in &mut merged {
        volume.recompute_mount_metadata();
    }
    merged.sort_by_key(|volume| {
        volume
            .canonical_mount_path()
            .as_deref()
            .map(mount_key)
            .unwrap_or_else(|| volume.volume_id.to_lowercase())
    });
    merged
}

fn records_represent_same_volume(left: &VolumeInfo, right: &VolumeInfo) -> bool {
    if left.volume_id.eq_ignore_ascii_case(&right.volume_id) {
        return true;
    }
    if let (Some(left_guid), Some(right_guid)) = (&left.volume_guid_path, &right.volume_guid_path) {
        if left_guid.eq_ignore_ascii_case(right_guid) {
            return true;
        }
    }
    let left_aliases: HashSet<String> = left.mount_paths.iter().map(|p| mount_key(p)).collect();
    right.mount_paths.iter().any(|path| {
        left_aliases.contains(&mount_key(path))
            && left.drive_type == right.drive_type
            && (left.fs_type.is_empty()
                || right.fs_type.is_empty()
                || left.fs_type.eq_ignore_ascii_case(&right.fs_type))
    })
}

fn merge_into(existing: &mut VolumeInfo, incoming: VolumeInfo) {
    existing.mount_paths.extend(incoming.mount_paths);
    existing
        .discovery_sources
        .extend(incoming.discovery_sources);
    existing.discovery_sources.sort();
    existing.discovery_sources.dedup();
    if existing.volume_guid_path.is_none() {
        existing.volume_guid_path = incoming.volume_guid_path;
        existing.volume_id = incoming.volume_id;
    }
    if existing.label.is_empty() {
        existing.label = incoming.label;
    }
    if existing.fs_type.is_empty() {
        existing.fs_type = incoming.fs_type;
    }
    if existing.drive_type == 0 {
        existing.drive_type = incoming.drive_type;
    }
    existing.accessible |= incoming.accessible;
    existing.recompute_mount_metadata();
}

fn inspect_guid_volume(
    volume_guid_path: &str,
    source: &str,
    logical_alias: Option<PathBuf>,
) -> Option<VolumeInfo> {
    let mut mount_paths = match get_volume_mount_paths(volume_guid_path) {
        Ok(paths) => paths,
        Err(error) => {
            debug_api_error("GetVolumePathNamesForVolumeNameW", volume_guid_path, &error);
            Vec::new()
        }
    };
    if let Some(alias) = logical_alias {
        mount_paths.push(alias);
    }
    let mount_paths = normalize_mount_paths(&mount_paths);
    let canonical =
        canonical_mount_path_with(&mount_paths, |path| path.try_exists().unwrap_or(false))?;
    let (drive_type, label, fs_type) = inspect_mount_metadata(&canonical)?;
    let mut volume = VolumeInfo {
        volume_id: stable_guid_id(volume_guid_path)?,
        volume_guid_path: Some(volume_guid_path.to_string()),
        discovery_sources: vec![source.to_string()],
        drive_letter: None,
        mount_paths,
        drive_type,
        label,
        fs_type,
        accessible: canonical.try_exists().unwrap_or(false),
    };
    volume.recompute_mount_metadata();
    Some(volume)
}

fn inspect_logical_fallback(root: &Path) -> Option<VolumeInfo> {
    let (drive_type, label, fs_type) = inspect_mount_metadata(root)?;
    let mut serial_number = 0u32;
    let wide_root = wide_null(root.as_os_str().to_string_lossy().as_ref());
    let mut max_component_len = 0u32;
    let mut flags = 0u32;
    let _ = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide_root.as_ptr()),
            None,
            Some(&mut serial_number),
            Some(&mut max_component_len),
            Some(&mut flags),
            None,
        )
    };
    let drive_letter = drive_letter_from_mount_path(root);
    let volume_id = if serial_number != 0 {
        format!("vol_{serial_number:08x}")
    } else if let Some(letter) = &drive_letter {
        format!("drive_{}", letter.to_ascii_lowercase())
    } else {
        format!(
            "path_{}",
            mount_key(root).replace(['\\', '/', ':', '?'], "_")
        )
    };
    let mut volume = VolumeInfo {
        volume_id,
        volume_guid_path: None,
        discovery_sources: vec!["logical-drive-fallback".to_string()],
        drive_letter,
        mount_paths: vec![root.to_path_buf()],
        drive_type,
        label,
        fs_type,
        accessible: root.try_exists().unwrap_or(false),
    };
    volume.recompute_mount_metadata();
    Some(volume)
}

fn inspect_mount_metadata(root: &Path) -> Option<(u32, String, String)> {
    // GetDriveTypeW and GetVolumeInformationW require a root path ending in a
    // separator, including mounted-folder roots.
    let root_text = root.as_os_str().to_string_lossy();
    let api_root = if root_text.ends_with(['\\', '/']) {
        root_text.into_owned()
    } else {
        format!("{root_text}\\")
    };
    let wide_root = wide_null(&api_root);
    let drive_type = unsafe { GetDriveTypeW(PCWSTR(wide_root.as_ptr())) };
    if !is_supported_drive_type(drive_type) {
        return None;
    }
    let mut volume_name_buf = [0u16; 260];
    let mut serial_number = 0u32;
    let mut max_component_len = 0u32;
    let mut flags = 0u32;
    let mut fs_name_buf = [0u16; 260];
    let result = unsafe {
        GetVolumeInformationW(
            PCWSTR(wide_root.as_ptr()),
            Some(&mut volume_name_buf),
            Some(&mut serial_number),
            Some(&mut max_component_len),
            Some(&mut flags),
            Some(&mut fs_name_buf),
        )
    };
    if let Err(error) = result {
        debug_api_error("GetVolumeInformationW", &root.to_string_lossy(), &error);
        if drive_type == DRIVE_CDROM {
            return None;
        }
    }
    Some((
        drive_type,
        wide_to_string(&volume_name_buf),
        wide_to_string(&fs_name_buf),
    ))
}

fn enumerate_volume_guids() -> Vec<String> {
    let mut output = Vec::new();
    let mut buffer = [0u16; 512];
    let handle = match unsafe { FindFirstVolumeW(&mut buffer) } {
        Ok(handle) => handle,
        Err(error) => {
            debug_api_error("FindFirstVolumeW", "", &error);
            return output;
        }
    };
    loop {
        let volume_guid = wide_to_string(&buffer);
        if !volume_guid.is_empty() {
            output.push(volume_guid);
        }
        if let Err(error) = unsafe { FindNextVolumeW(handle, &mut buffer) } {
            // ERROR_NO_MORE_FILES is the expected enumeration terminator, but
            // retaining the code in debug output makes partial discovery clear.
            debug_api_error("FindNextVolumeW", "", &error);
            break;
        }
    }
    let _ = unsafe { FindVolumeClose(handle) };
    output
}

fn volume_name_for_mount_point(root: &Path) -> Result<String, windows::core::Error> {
    let wide_root = wide_null(root.as_os_str().to_string_lossy().as_ref());
    let mut buffer = [0u16; 512];
    unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(wide_root.as_ptr()), &mut buffer) }?;
    Ok(wide_to_string(&buffer))
}

fn get_volume_mount_paths(volume_guid: &str) -> Result<Vec<PathBuf>, windows::core::Error> {
    let wide_volume = wide_null(volume_guid);
    let mut required = 0u32;
    let first = unsafe {
        GetVolumePathNamesForVolumeNameW(PCWSTR(wide_volume.as_ptr()), None, &mut required)
    };
    if required == 0 {
        first?;
        return Ok(Vec::new());
    }
    let mut buffer = vec![0u16; required as usize];
    unsafe {
        GetVolumePathNamesForVolumeNameW(
            PCWSTR(wide_volume.as_ptr()),
            Some(&mut buffer),
            &mut required,
        )
    }?;
    Ok(buffer
        .split(|unit| *unit == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| PathBuf::from(OsString::from_wide(slice)))
        .collect())
}

fn get_logical_drives() -> Vec<PathBuf> {
    let required = unsafe { GetLogicalDriveStringsW(None) };
    if required == 0 {
        return Vec::new();
    }
    let mut buffer = vec![0u16; required as usize + 1];
    let written = unsafe { GetLogicalDriveStringsW(Some(&mut buffer)) } as usize;
    if written == 0 || written > buffer.len() {
        return Vec::new();
    }
    buffer[..written]
        .split(|unit| *unit == 0)
        .filter(|slice| !slice.is_empty())
        .map(|slice| PathBuf::from(OsString::from_wide(slice)))
        .collect()
}

fn normalize_mount_paths(paths: &[PathBuf]) -> Vec<PathBuf> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();
    for path in paths {
        let path = normalize_mount_path(path);
        if seen.insert(mount_key(&path)) {
            normalized.push(path);
        }
    }
    normalized.sort_by_key(|path| mount_key(path));
    normalized
}

fn normalize_mount_path(path: &Path) -> PathBuf {
    let value = path.as_os_str().to_string_lossy().replace('/', "\\");
    let bytes = value.as_bytes();
    if bytes.len() >= 2 && bytes[0].is_ascii_alphabetic() && bytes[1] == b':' {
        let letter = (bytes[0] as char).to_ascii_uppercase();
        let suffix = value[2..].trim_end_matches('\\');
        if suffix.is_empty() {
            return PathBuf::from(format!("{letter}:\\"));
        }
        return PathBuf::from(format!("{letter}:{suffix}"));
    }
    if value.starts_with(r"\\") {
        return PathBuf::from(value.trim_end_matches('\\'));
    }
    PathBuf::from(value.trim_end_matches('\\'))
}

fn stable_guid_id(volume_guid_path: &str) -> Option<String> {
    let lower = volume_guid_path.to_ascii_lowercase();
    lower
        .strip_prefix(&GUID_PREFIX.to_ascii_lowercase())
        .and_then(|value| value.strip_suffix("}\\"))
        .filter(|value| !value.is_empty())
        .map(str::to_string)
}

fn component_count(path: &Path) -> usize {
    path.components().count()
}

fn mount_key(path: &Path) -> String {
    path.as_os_str().to_string_lossy().to_lowercase()
}

fn is_supported_drive_type(drive_type: u32) -> bool {
    matches!(
        drive_type,
        DRIVE_FIXED | DRIVE_REMOVABLE | DRIVE_RAMDISK | DRIVE_REMOTE | DRIVE_CDROM
    )
}

fn wide_null(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(Some(0)).collect()
}

fn wide_to_string(wide: &[u16]) -> String {
    let end = wide
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(wide.len());
    OsString::from_wide(&wide[..end])
        .to_string_lossy()
        .into_owned()
}

fn debug_api_error(api: &str, input: &str, error: &windows::core::Error) {
    if super::catalog_debug_enabled() {
        eprintln!(
            "{}",
            serde_json::json!({
                "event": "volume_api_error",
                "api": api,
                "input": input,
                "error": error.to_string(),
                "hresult": format!("0x{:08x}", error.code().0 as u32),
            })
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use windows::Win32::System::WindowsProgramming::DRIVE_FIXED;

    fn volume(id: &str, guid: Option<&str>, mounts: &[&str]) -> VolumeInfo {
        let mut volume = VolumeInfo {
            volume_id: id.into(),
            volume_guid_path: guid.map(str::to_string),
            discovery_sources: vec!["test".into()],
            drive_letter: None,
            mount_paths: mounts.iter().map(PathBuf::from).collect(),
            drive_type: DRIVE_FIXED,
            label: "Data".into(),
            fs_type: "NTFS".into(),
            accessible: true,
        };
        volume.recompute_mount_metadata();
        volume
    }

    fn canonical(mounts: &[&str]) -> PathBuf {
        canonical_mount_path_with(
            &mounts.iter().map(PathBuf::from).collect::<Vec<_>>(),
            |_| true,
        )
        .unwrap()
    }

    #[test]
    fn canonical_mount_prefers_drive_root_regardless_of_input_order() {
        for mounts in [
            vec![r"C:\Mounts\Data\", r"D:\"],
            vec![r"D:\", r"C:\Mounts\Data\"],
        ] {
            assert_eq!(canonical(&mounts), PathBuf::from(r"D:\"));
        }
    }

    #[test]
    fn canonical_mounted_folder_selection_is_deterministic() {
        let first = canonical(&[r"C:\Mounts\Long\Data\", r"C:\Data\"]);
        let second = canonical(&[r"C:\Data\", r"C:\Mounts\Long\Data\"]);
        assert_eq!(first, PathBuf::from(r"C:\Data"));
        assert_eq!(first, second);
    }

    #[test]
    fn aliases_are_deduplicated_case_insensitively() {
        let paths = normalize_mount_paths(&[
            PathBuf::from(r"d:\"),
            PathBuf::from(r"D:\"),
            PathBuf::from(r"D:/"),
        ]);
        assert_eq!(paths, vec![PathBuf::from(r"D:\")]);
    }

    #[test]
    fn drive_letter_requires_a_true_root() {
        assert_eq!(
            drive_letter_from_mount_path(Path::new(r"d:\")),
            Some("D:".into())
        );
        assert_eq!(drive_letter_from_mount_path(Path::new(r"D:\Mounted")), None);
        assert_eq!(drive_letter_from_mount_path(Path::new("not:a-root")), None);
    }

    #[test]
    fn logical_fallback_recomputes_its_drive_letter() {
        let mut fallback = volume("drive_d:", None, &[r"D:\"]);
        fallback.drive_letter = None;
        fallback.recompute_mount_metadata();
        assert_eq!(fallback.drive_letter.as_deref(), Some("D:"));
    }

    #[test]
    fn guid_and_logical_records_merge_but_distinct_drives_do_not() {
        let guid = r"\\?\Volume{11111111-1111-1111-1111-111111111111}\";
        let records = vec![
            volume("guid-d", Some(guid), &[r"D:\", r"C:\Mounts\Data\"]),
            volume("guid-d", Some(guid), &[r"d:\"]),
            volume("guid-c", None, &[r"C:\"]),
        ];
        let merged = merge_volume_records(records);
        assert_eq!(merged.len(), 2);
        let secondary = merged
            .iter()
            .find(|item| item.volume_id == "guid-d")
            .unwrap();
        assert_eq!(
            secondary.canonical_mount_path(),
            Some(PathBuf::from(r"D:\"))
        );
        assert_eq!(secondary.mount_paths.len(), 2);
    }

    #[test]
    fn inaccessible_record_does_not_remove_other_volumes() {
        let mut offline = volume("offline", None, &[r"E:\"]);
        offline.accessible = false;
        let merged = merge_volume_records(vec![
            offline,
            volume("system", None, &[r"C:\"]),
            volume("data", None, &[r"D:\"]),
        ]);
        assert_eq!(merged.len(), 3);
        assert!(merged.iter().any(|item| item.volume_id == "system"));
        assert!(merged.iter().any(|item| item.volume_id == "data"));
    }

    #[test]
    #[ignore = "set PRISM_TEST_SECONDARY_DRIVE to an available drive root"]
    fn indexes_and_searches_environment_selected_drive() {
        use std::sync::atomic::AtomicBool;
        use std::sync::Arc;
        use std::time::{SystemTime, UNIX_EPOCH};

        let root = PathBuf::from(
            std::env::var("PRISM_TEST_SECONDARY_DRIVE")
                .expect("PRISM_TEST_SECONDARY_DRIVE must be set, for example D:\\"),
        );
        let volume = discover_volumes()
            .into_iter()
            .find(|volume| {
                volume.normalized_mount_paths().iter().any(|path| {
                    path.as_os_str()
                        .to_string_lossy()
                        .eq_ignore_ascii_case(&root.as_os_str().to_string_lossy())
                })
            })
            .expect("requested drive must appear in discover_volumes()");
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let probe_base = root.join("PrismDriveProbe");
        let probe_dir = probe_base.join(format!("catalog-test-{unique}"));
        std::fs::create_dir_all(&probe_dir).expect("create probe directory");
        let probe_name = format!("prism_secondary_drive_probe_{unique}.txt");
        let probe_path = probe_dir.join(&probe_name);
        std::fs::write(&probe_path, "Prism secondary-drive catalog probe")
            .expect("create probe file");

        let db_dir = std::env::temp_dir().join(format!("prism-drive-test-{unique}"));
        std::fs::create_dir_all(&db_dir).unwrap();
        let db = Arc::new(super::super::db::Database::open(&db_dir.join("catalog.db")).unwrap());
        db.upsert_volume(&volume, super::super::types::VolumeState::Indexing)
            .unwrap();
        super::super::scanner::scan_volume(
            &probe_dir,
            &volume.volume_id,
            1,
            db.clone(),
            &db_dir,
            Arc::new(AtomicBool::new(false)),
            |_| {},
        )
        .unwrap();
        let candidates = db.search_candidates(&probe_name, 100).unwrap();
        assert!(candidates.iter().any(|candidate| {
            Path::new(&candidate.display_path) == probe_path
                && Path::new(&candidate.display_path).exists()
        }));

        let _ = std::fs::remove_dir_all(&probe_dir);
        if probe_base
            .read_dir()
            .map(|mut entries| entries.next().is_none())
            .unwrap_or(false)
        {
            let _ = std::fs::remove_dir(&probe_base);
        }
        let _ = std::fs::remove_dir_all(&db_dir);
    }
}
