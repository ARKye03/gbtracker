//! Loaded issue details with markdown parsed once, and the cache that keeps them across pane opens.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::rc::Rc;

use iced::widget::markdown;

use crate::gitbug::IssueDetail;

#[derive(Debug)]
pub struct LoadedDetail {
    pub detail: IssueDetail,
    pub body: markdown::Content,
    pub comments: Vec<markdown::Content>,
}

impl LoadedDetail {
    pub fn new(detail: IssueDetail) -> Self {
        let body = markdown::Content::parse(&detail.body);
        let comments = detail
            .comments
            .iter()
            .map(|c| markdown::Content::parse(&c.message))
            .collect();
        LoadedDetail {
            detail,
            body,
            comments,
        }
    }
}

/// One entry per (repo, id); a different `edit_time` replaces it, so stale details never linger.
#[derive(Debug, Default)]
pub struct DetailCache {
    entries: HashMap<(PathBuf, String), (String, Rc<LoadedDetail>)>,
}

impl DetailCache {
    pub fn get(&self, root: &Path, id: &str, edited: &str) -> Option<Rc<LoadedDetail>> {
        let (cached_edit, detail) = self.entries.get(&(root.to_owned(), id.to_owned()))?;
        (cached_edit == edited).then(|| Rc::clone(detail))
    }

    pub fn insert(&mut self, root: PathBuf, id: String, edited: String, detail: Rc<LoadedDetail>) {
        self.entries.insert((root, id), (edited, detail));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gitbug::parse_show;

    fn loaded() -> Rc<LoadedDetail> {
        let json = include_str!("../tests/fixtures/show.json");
        Rc::new(LoadedDetail::new(parse_show(json).unwrap()))
    }

    #[test]
    fn markdown_is_parsed_up_front() {
        let d = loaded();
        assert!(!d.body.items().is_empty());
        assert_eq!(d.comments.len(), d.detail.comments.len());
    }

    #[test]
    fn cache_hits_only_on_matching_repo_id_and_edit_time() {
        let mut cache = DetailCache::default();
        let root = PathBuf::from("/r/a");
        cache.insert(root.clone(), "id".into(), "t1".into(), loaded());

        assert!(cache.get(&root, "id", "t1").is_some());
        assert!(cache.get(&root, "id", "t2").is_none(), "newer edit misses");
        assert!(cache.get(Path::new("/r/b"), "id", "t1").is_none());
        assert!(cache.get(&root, "other", "t1").is_none());

        cache.insert(root.clone(), "id".into(), "t2".into(), loaded());
        assert!(cache.get(&root, "id", "t1").is_none(), "old entry replaced");
        assert!(cache.get(&root, "id", "t2").is_some());
    }
}
