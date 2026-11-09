//! Pure issue filtering and the active-work ordering. Nothing here touches the UI.

use std::collections::{BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use crate::gitbug::{Issue, Kind, State, Status};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StatusFilter {
    #[default]
    Open,
    Closed,
    All,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StateFilter {
    #[default]
    Any,
    InProgress,
    Blocked,
}

/// Empty `kinds`, `repos` and `labels` mean "no restriction"; selected labels must all be present.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Filter {
    pub kinds: Vec<Kind>,
    pub status: StatusFilter,
    pub state: StateFilter,
    pub repos: BTreeSet<PathBuf>,
    pub labels: BTreeSet<String>,
    text: String,
}

impl Filter {
    pub fn text(&self) -> &str {
        &self.text
    }

    /// Stored trimmed and lowercased so `matches` never has to lowercase the query.
    pub fn set_text(&mut self, text: &str) {
        self.text = text.trim().to_lowercase();
    }

    pub fn toggle_kind(&mut self, kind: Kind) {
        match self.kinds.iter().position(|k| *k == kind) {
            Some(pos) => {
                self.kinds.remove(pos);
            }
            None => self.kinds.push(kind),
        }
    }

    pub fn is_default(&self) -> bool {
        *self == Filter::default()
    }
}

/// Lowercased title and human id, built once when an issue is loaded.
pub fn search_key(issue: &Issue) -> String {
    format!("{}\n{}", issue.title, issue.human_id).to_lowercase()
}

pub fn matches(issue: &Issue, repo: &Path, key: &str, filter: &Filter) -> bool {
    let status_ok = match filter.status {
        StatusFilter::Open => issue.status == Status::Open,
        StatusFilter::Closed => issue.status == Status::Closed,
        StatusFilter::All => true,
    };
    let state_ok = match filter.state {
        StateFilter::Any => true,
        StateFilter::InProgress => issue.state == Some(State::InProgress),
        StateFilter::Blocked => issue.state == Some(State::Blocked),
    };
    status_ok
        && state_ok
        && (filter.kinds.is_empty()
            || issue
                .kind
                .as_ref()
                .is_some_and(|k| filter.kinds.contains(k)))
        && (filter.repos.is_empty() || filter.repos.contains(repo))
        && filter.labels.iter().all(|l| issue.labels.contains(l))
        && (filter.text.is_empty() || key.contains(&filter.text))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placed {
    pub index: usize,
    pub depth: usize,
}

fn urgent(issue: &Issue) -> bool {
    matches!(issue.state, Some(State::InProgress | State::Blocked))
}

/// Active work order: in-progress/blocked groups first, children nested under their epic by layer.
/// Each entry pairs a repo index with its issue, since clones of one project share git-bug ids.
/// Indices refer to `issues`; an issue whose epic is not in the slice stays top level.
pub fn active_order(entries: &[(usize, &Issue)]) -> Vec<Placed> {
    let issues: Vec<&Issue> = entries.iter().map(|&(_, i)| i).collect();
    let epic_at: HashMap<(usize, &str), usize> = entries
        .iter()
        .enumerate()
        .filter(|(_, (_, i))| i.kind == Some(Kind::Epic))
        .map(|(pos, &(repo, i))| ((repo, i.id.as_str()), pos))
        .collect();

    let mut children: HashMap<usize, Vec<usize>> = HashMap::new();
    let mut tops = Vec::new();
    for (pos, &(repo, issue)) in entries.iter().enumerate() {
        let parent = (issue.kind != Some(Kind::Epic))
            .then(|| {
                issue
                    .epic
                    .as_deref()
                    .and_then(|id| epic_at.get(&(repo, id)))
            })
            .flatten();
        match parent {
            Some(&epic) => children.entry(epic).or_default().push(pos),
            None => tops.push(pos),
        }
    }

    for kids in children.values_mut() {
        kids.sort_by_key(|&k| (issues[k].layer.unwrap_or(u32::MAX), !urgent(issues[k])));
    }
    let group_urgent = |top: usize| {
        urgent(issues[top])
            || children
                .get(&top)
                .is_some_and(|kids| kids.iter().any(|&k| urgent(issues[k])))
    };
    tops.sort_by_key(|&t| !group_urgent(t));

    let mut out = Vec::with_capacity(issues.len());
    for top in tops {
        out.push(Placed {
            index: top,
            depth: 0,
        });
        for &kid in children.get(&top).into_iter().flatten() {
            out.push(Placed {
                index: kid,
                depth: 1,
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn issue(id: &str, title: &str) -> Issue {
        Issue {
            id: id.repeat(8),
            human_id: id.repeat(7),
            title: title.into(),
            status: Status::Open,
            kind: None,
            epic: None,
            state: None,
            layer: None,
            labels: vec![],
            author: "me".into(),
            created: String::new(),
            edited: String::new(),
            comment_count: 1,
        }
    }

    fn hit(issue: &Issue, filter: &Filter) -> bool {
        matches(issue, Path::new("/r/a"), &search_key(issue), filter)
    }

    #[test]
    fn default_filter_is_open_only() {
        let mut closed = issue("a", "t");
        closed.status = Status::Closed;
        let filter = Filter::default();
        assert!(hit(&issue("a", "t"), &filter));
        assert!(!hit(&closed, &filter));
        assert!(filter.is_default());
    }

    #[test]
    fn status_dimension() {
        let mut closed = issue("a", "t");
        closed.status = Status::Closed;
        let mut f = Filter {
            status: StatusFilter::Closed,
            ..Filter::default()
        };
        assert!(hit(&closed, &f));
        assert!(!hit(&issue("a", "t"), &f));
        f.status = StatusFilter::All;
        assert!(hit(&closed, &f) && hit(&issue("a", "t"), &f));
    }

    #[test]
    fn kind_dimension() {
        let mut task = issue("a", "t");
        task.kind = Some(Kind::Task);
        let mut f = Filter::default();
        f.toggle_kind(Kind::Bug);
        assert!(!hit(&task, &f));
        assert!(!hit(&issue("b", "t"), &f), "unkinded issue excluded");
        f.toggle_kind(Kind::Task);
        assert!(hit(&task, &f));
        f.toggle_kind(Kind::Task);
        f.toggle_kind(Kind::Bug);
        assert!(f.kinds.is_empty());
    }

    #[test]
    fn state_dimension() {
        let mut blocked = issue("a", "t");
        blocked.state = Some(State::Blocked);
        let mut f = Filter {
            state: StateFilter::Blocked,
            ..Filter::default()
        };
        assert!(hit(&blocked, &f));
        assert!(!hit(&issue("b", "t"), &f));
        f.state = StateFilter::InProgress;
        assert!(!hit(&blocked, &f));
    }

    #[test]
    fn repo_dimension() {
        let i = issue("a", "t");
        let mut f = Filter::default();
        f.repos.insert("/r/b".into());
        assert!(!hit(&i, &f));
        f.repos.insert("/r/a".into());
        assert!(hit(&i, &f));
    }

    #[test]
    fn label_dimension_requires_all() {
        let mut i = issue("a", "t");
        i.labels = vec!["ui".into(), "perf".into()];
        let mut f = Filter::default();
        f.labels.insert("ui".into());
        assert!(hit(&i, &f));
        f.labels.insert("docs".into());
        assert!(!hit(&i, &f));
    }

    #[test]
    fn text_matches_title_and_human_id_case_insensitively() {
        let i = issue("c", "Fix The Parser");
        let mut f = Filter::default();
        f.set_text("  PARSER ");
        assert_eq!(f.text(), "parser");
        assert!(hit(&i, &f));
        f.set_text("CCCC");
        assert!(hit(&i, &f));
        f.set_text("lexer");
        assert!(!hit(&i, &f));
        f.set_text("");
        assert!(hit(&i, &f));
    }

    #[test]
    fn combined_dimensions_all_apply() {
        let mut i = issue("a", "Fix parser");
        i.kind = Some(Kind::Bug);
        i.state = Some(State::InProgress);
        i.labels = vec!["ui".into()];
        let mut f = Filter {
            state: StateFilter::InProgress,
            ..Filter::default()
        };
        f.toggle_kind(Kind::Bug);
        f.repos.insert("/r/a".into());
        f.labels.insert("ui".into());
        f.set_text("parser");
        assert!(hit(&i, &f));
        f.set_text("lexer");
        assert!(!hit(&i, &f));
    }

    fn typed(id: &str, kind: Kind, state: Option<State>) -> Issue {
        let mut i = issue(id, id);
        i.kind = Some(kind);
        i.state = state;
        i
    }

    fn child(id: &str, epic: &Issue, layer: u32, state: Option<State>) -> Issue {
        let mut i = typed(id, Kind::Task, state);
        i.epic = Some(epic.id.clone());
        i.layer = Some(layer);
        i
    }

    #[test]
    fn active_puts_urgent_groups_first_and_nests_children_by_layer() {
        let calm = typed("a", Kind::Epic, None);
        let busy = typed("b", Kind::Epic, None);
        let calm_l2 = child("c", &calm, 2, None);
        let calm_l1 = child("d", &calm, 1, None);
        let busy_l2 = child("e", &busy, 2, Some(State::InProgress));
        let busy_l1 = child("f", &busy, 1, None);
        let loose = typed("g", Kind::Bug, Some(State::Blocked));
        let plain = typed("h", Kind::Bug, None);
        let all = [
            &calm, &calm_l2, &calm_l1, &busy, &busy_l2, &busy_l1, &plain, &loose,
        ];
        let entries: Vec<(usize, &Issue)> = all.iter().map(|&i| (0, i)).collect();

        let order: Vec<(char, usize)> = active_order(&entries)
            .into_iter()
            .map(|p| (all[p.index].title.chars().next().unwrap(), p.depth))
            .collect();
        assert_eq!(
            order,
            [
                ('b', 0),
                ('f', 1),
                ('e', 1),
                ('g', 0),
                ('a', 0),
                ('d', 1),
                ('c', 1),
                ('h', 0),
            ]
        );
    }

    #[test]
    fn active_keeps_orphans_and_epics_of_epics_top_level() {
        let missing = typed("a", Kind::Epic, None);
        let orphan = child("b", &missing, 1, None);
        let mut sub = typed("c", Kind::Epic, None);
        sub.epic = Some(orphan.id.clone());
        let order = active_order(&[(0, &orphan), (0, &sub)]);
        assert_eq!(order.len(), 2);
        assert!(order.iter().all(|p| p.depth == 0));
    }

    #[test]
    fn active_does_not_merge_epics_shared_by_clones() {
        let epic = typed("a", Kind::Epic, None);
        let kid = child("b", &epic, 1, None);
        let order = active_order(&[(0, &epic), (0, &kid), (1, &epic), (1, &kid)]);
        let shape: Vec<(usize, usize)> = order.iter().map(|p| (p.index, p.depth)).collect();
        assert_eq!(shape, [(0, 0), (1, 1), (2, 0), (3, 1)]);
    }
}
