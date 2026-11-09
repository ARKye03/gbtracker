//! Maps keyed by `(repo root, issue id)` that can be queried with borrowed keys.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct IssueMap<V> {
    roots: HashMap<PathBuf, HashMap<String, V>>,
}

impl<V> Default for IssueMap<V> {
    fn default() -> Self {
        Self {
            roots: HashMap::new(),
        }
    }
}

impl<V> IssueMap<V> {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self, root: &Path, id: &str) -> Option<&V> {
        self.roots.get(root)?.get(id)
    }

    pub fn get_mut(&mut self, root: &Path, id: &str) -> Option<&mut V> {
        self.roots.get_mut(root)?.get_mut(id)
    }

    pub fn contains(&self, root: &Path, id: &str) -> bool {
        self.get(root, id).is_some()
    }

    pub fn insert(&mut self, root: PathBuf, id: String, value: V) -> Option<V> {
        self.roots.entry(root).or_default().insert(id, value)
    }

    pub fn remove(&mut self, root: &Path, id: &str) -> Option<V> {
        let ids = self.roots.get_mut(root)?;
        let removed = ids.remove(id);
        if ids.is_empty() {
            self.roots.remove(root);
        }
        removed
    }

    pub fn is_empty(&self) -> bool {
        self.roots.is_empty()
    }
}

impl<V: Default> IssueMap<V> {
    /// Inserts a default value unless one is present; clones the key only on insert.
    pub fn ensure(&mut self, root: &Path, id: &str) {
        if self.contains(root, id) {
            return;
        }
        self.insert(root.to_owned(), id.to_owned(), V::default());
    }
}

#[derive(Debug, Clone, Default)]
pub struct IssueSet(IssueMap<()>);

impl IssueSet {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn contains(&self, root: &Path, id: &str) -> bool {
        self.0.contains(root, id)
    }

    /// Returns whether the key was newly added.
    pub fn insert(&mut self, root: PathBuf, id: String) -> bool {
        self.0.insert(root, id, ()).is_none()
    }

    /// Returns whether the key was present.
    pub fn remove(&mut self, root: &Path, id: &str) -> bool {
        self.0.remove(root, id).is_some()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root(name: &str) -> PathBuf {
        PathBuf::from(format!("/repos/{name}"))
    }

    #[test]
    fn lookups_borrow_and_separate_roots() {
        let mut map = IssueMap::new();
        map.insert(root("a"), "x".into(), 1);
        map.insert(root("b"), "x".into(), 2);
        assert_eq!(map.get(&root("a"), "x"), Some(&1));
        assert_eq!(map.get(&root("b"), "x"), Some(&2));
        assert!(!map.contains(&root("a"), "y"));
        assert!(!map.contains(&root("c"), "x"));
    }

    #[test]
    fn insert_replaces_and_remove_prunes_empty_roots() {
        let mut map = IssueMap::new();
        assert_eq!(map.insert(root("a"), "x".into(), 1), None);
        assert_eq!(map.insert(root("a"), "x".into(), 2), Some(1));
        *map.get_mut(&root("a"), "x").unwrap() += 1;
        assert_eq!(map.remove(&root("a"), "x"), Some(3));
        assert_eq!(map.remove(&root("a"), "x"), None);
        assert!(map.is_empty());
    }

    #[test]
    fn ensure_keeps_an_existing_value() {
        let mut map: IssueMap<String> = IssueMap::new();
        map.insert(root("a"), "x".into(), "kept".into());
        map.ensure(&root("a"), "x");
        map.ensure(&root("a"), "y");
        assert_eq!(map.get(&root("a"), "x").map(String::as_str), Some("kept"));
        assert_eq!(map.get(&root("a"), "y").map(String::as_str), Some(""));
    }

    #[test]
    fn set_reports_membership_changes() {
        let mut set = IssueSet::new();
        assert!(set.insert(root("a"), "x".into()));
        assert!(!set.insert(root("a"), "x".into()));
        assert!(set.contains(&root("a"), "x"));
        assert!(set.remove(&root("a"), "x"));
        assert!(!set.remove(&root("a"), "x"));
        assert!(set.is_empty());
    }
}
