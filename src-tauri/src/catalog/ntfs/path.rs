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

#[cfg(test)]
mod tests {
    use std::collections::HashMap;

    use super::*;

    fn node(frn: u64, parent: u64, name: &str) -> PathNode {
        PathNode {
            frn,
            parent_frn: parent,
            name: name.into(),
        }
    }

    fn resolve(nodes: &HashMap<u64, PathNode>, frn: u64) -> PathResolution {
        resolve_path(frn, "C:\\", |id| nodes.get(&id).cloned())
    }

    #[test]
    fn reconstructs_frn_parent_graph_iteratively() {
        let nodes = HashMap::from([
            (5, node(5, 5, ".")),
            (10, node(10, 5, "Projects")),
            (11, node(11, 10, "Prism")),
            (12, node(12, 11, "main.rs")),
        ]);
        assert_eq!(
            resolve(&nodes, 12),
            PathResolution::Resolved(PathBuf::from("C:\\Projects\\Prism\\main.rs"))
        );
    }

    #[test]
    fn directory_rename_changes_descendant_path_without_touching_child() {
        let mut nodes = HashMap::from([
            (5, node(5, 5, ".")),
            (10, node(10, 5, "Prism")),
            (11, node(11, 10, "main.rs")),
        ]);
        nodes.get_mut(&10).unwrap().name = "SuperPrism".into();
        assert_eq!(
            resolve(&nodes, 11),
            PathResolution::Resolved(PathBuf::from("C:\\SuperPrism\\main.rs"))
        );
    }

    #[test]
    fn directory_and_file_moves_only_change_the_moved_parent() {
        let mut nodes = HashMap::from([
            (5, node(5, 5, ".")),
            (10, node(10, 5, "A")),
            (20, node(20, 5, "B")),
            (11, node(11, 10, "src")),
            (12, node(12, 11, "main.rs")),
        ]);
        nodes.get_mut(&11).unwrap().parent_frn = 20;
        assert_eq!(
            resolve(&nodes, 12),
            PathResolution::Resolved(PathBuf::from("C:\\B\\src\\main.rs"))
        );
        nodes.get_mut(&12).unwrap().parent_frn = 10;
        assert_eq!(
            resolve(&nodes, 12),
            PathResolution::Resolved(PathBuf::from("C:\\A\\main.rs"))
        );
    }

    #[test]
    fn orphan_and_cycle_are_bounded_errors() {
        let orphan = HashMap::from([(10, node(10, 99, "lost.txt"))]);
        assert_eq!(
            resolve(&orphan, 10),
            PathResolution::Orphaned { missing_frn: 99 }
        );
        let cycle = HashMap::from([(10, node(10, 11, "a")), (11, node(11, 10, "b"))]);
        assert_eq!(resolve(&cycle, 10), PathResolution::Cycle { frn: 10 });
    }
}
