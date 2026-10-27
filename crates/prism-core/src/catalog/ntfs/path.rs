use std::collections::HashSet;
use std::path::PathBuf;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathNode {
    pub frn: u64,
    pub parent_frn: u64,
    pub name: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PathResolution {
    Resolved(PathBuf),
    Orphaned { missing_frn: u64 },
    Cycle { frn: u64 },
    DepthExceeded,
}

const MAX_PATH_DEPTH: usize = 1024;

pub fn resolve_path(
    start_frn: u64,
    mount_path: &str,
    mut lookup: impl FnMut(u64) -> Option<PathNode>,
) -> PathResolution {
    let mut current = start_frn;
    let mut seen = HashSet::new();
    let mut components = Vec::new();

    for _ in 0..MAX_PATH_DEPTH {
        if !seen.insert(current) {
            return PathResolution::Cycle { frn: current };
        }
        let Some(node) = lookup(current) else {
            return PathResolution::Orphaned {
                missing_frn: current,
            };
        };
        if node.parent_frn == node.frn {
            let mut path = PathBuf::from(mount_path);
            for component in components.iter().rev() {
                path.push(component);
            }
            return PathResolution::Resolved(path);
        }
        if !node.name.is_empty() && node.name != "." {
            components.push(node.name);
        }
        current = node.parent_frn;
    }
    PathResolution::DepthExceeded
}
