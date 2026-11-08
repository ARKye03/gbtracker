//! Epic summaries and per-repo counts derived from loaded issues. Nothing here touches the UI.

use std::cmp::Reverse;
use std::collections::HashMap;
use std::path::Path;

use crate::filter::{self, Filter};
use crate::gitbug::{Issue, Kind, State, Status};

/// Positions refer to the `repos` slice given to `summarize` and that repo's `issues`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpicSummary {
    pub repo: usize,
    pub epic: usize,
    /// Ordered by layer; unlayered children last.
    pub children: Vec<usize>,
    pub closed: usize,
    pub total: usize,
    pub in_progress: usize,
    pub blocked: usize,
    /// Raw timestamp of the newest edit among the epic and its children.
    pub last_activity: String,
}

impl EpicSummary {
    pub fn ratio(&self) -> f32 {
        if self.total == 0 {
            0.0
        } else {
            self.closed as f32 / self.total as f32
        }
    }
}

/// Children are matched per repo: clones of one project share git-bug ids and must not merge.
/// `filter` applies to the epic itself; its children are always counted in full.
/// Most recently active epics come first.
pub fn summarize(repos: &[(&Path, &[Issue])], filter: &Filter) -> Vec<EpicSummary> {
    let mut out: Vec<(Option<i64>, EpicSummary)> = Vec::new();
    for (repo, &(root, issues)) in repos.iter().enumerate() {
        let mut kids: HashMap<&str, Vec<usize>> = HashMap::new();
        for (pos, issue) in issues.iter().enumerate() {
            if issue.kind == Some(Kind::Epic) {
                continue;
            }
            if let Some(epic) = issue.epic.as_deref() {
                kids.entry(epic).or_default().push(pos);
            }
        }

        for (pos, epic) in issues
            .iter()
            .enumerate()
            .filter(|(_, i)| i.kind == Some(Kind::Epic))
        {
            if !filter::matches(epic, root, &filter::search_key(epic), filter) {
                continue;
            }
            let mut children = kids.get(epic.id.as_str()).cloned().unwrap_or_default();
            children.sort_by_key(|&c| issues[c].layer.unwrap_or(u32::MAX));

            let open = |c: &&usize| issues[**c].status == Status::Open;
            let in_state = |s: State| {
                children
                    .iter()
                    .filter(open)
                    .filter(|&&c| issues[c].state == Some(s.clone()))
                    .count()
            };
            let closed = children
                .iter()
                .filter(|&&c| issues[c].status == Status::Closed)
                .count();

            let (key, last_activity) = std::iter::once(epic)
                .chain(children.iter().map(|&c| &issues[c]))
                .map(|i| (epoch_secs(&i.edited), i.edited.as_str()))
                .max_by_key(|&(secs, _)| secs)
                .map_or((None, String::new()), |(secs, raw)| (secs, raw.to_owned()));

            out.push((
                key,
                EpicSummary {
                    repo,
                    epic: pos,
                    closed,
                    total: children.len(),
                    in_progress: in_state(State::InProgress),
                    blocked: in_state(State::Blocked),
                    children,
                    last_activity,
                },
            ));
        }
    }
    out.sort_by_key(|(key, _)| Reverse(*key));
    out.into_iter().map(|(_, summary)| summary).collect()
}

/// Non-epic issues labelled with `epic`'s id, ordered by layer; unlayered last.
pub fn children_of(issues: &[Issue], epic: &Issue) -> Vec<usize> {
    if epic.kind != Some(Kind::Epic) {
        return Vec::new();
    }
    let mut kids: Vec<usize> = issues
        .iter()
        .enumerate()
        .filter(|(_, i)| i.kind != Some(Kind::Epic) && i.epic.as_deref() == Some(epic.id.as_str()))
        .map(|(pos, _)| pos)
        .collect();
    kids.sort_by_key(|&c| issues[c].layer.unwrap_or(u32::MAX));
    kids
}

pub fn parent_of<'a>(issues: &'a [Issue], issue: &Issue) -> Option<&'a Issue> {
    let id = issue.epic.as_deref()?;
    issues
        .iter()
        .find(|i| i.kind == Some(Kind::Epic) && i.id == id)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct RepoCounts {
    pub epics: usize,
    pub tasks: usize,
    pub bugs: usize,
    pub in_progress: usize,
    pub blocked: usize,
}

impl RepoCounts {
    pub fn open(&self) -> usize {
        self.epics + self.tasks + self.bugs
    }
}

/// Counts open issues only; in-progress and blocked cover every open kind, including unkinded issues.
pub fn count_open(issues: &[Issue]) -> RepoCounts {
    let mut counts = RepoCounts::default();
    for issue in issues.iter().filter(|i| i.status == Status::Open) {
        match issue.kind {
            Some(Kind::Epic) => counts.epics += 1,
            Some(Kind::Task) => counts.tasks += 1,
            Some(Kind::Bug) => counts.bugs += 1,
            _ => {}
        }
        match issue.state {
            Some(State::InProgress) => counts.in_progress += 1,
            Some(State::Blocked) => counts.blocked += 1,
            _ => {}
        }
    }
    counts
}

/// Seconds since the Unix epoch for an RFC 3339 timestamp, so instants in different UTC offsets compare correctly.
pub fn epoch_secs(rfc3339: &str) -> Option<i64> {
    let num = |range: std::ops::Range<usize>| rfc3339.get(range)?.parse::<i64>().ok();
    let (year, month, day) = (num(0..4)?, num(5..7)?, num(8..10)?);
    let (hour, minute, second) = (num(11..13)?, num(14..16)?, num(17..19)?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    let mut tail = rfc3339.get(19..)?;
    if let Some(frac) = tail.strip_prefix('.') {
        tail = frac.trim_start_matches(|c: char| c.is_ascii_digit());
    }
    let offset = match tail {
        "Z" | "z" => 0,
        _ => {
            let sign = match tail.get(..1)? {
                "+" => 1,
                "-" => -1,
                _ => return None,
            };
            let h: i64 = tail.get(1..3)?.parse().ok()?;
            let m: i64 = tail.get(4..6)?.parse().ok()?;
            sign * (h * 3600 + m * 60)
        }
    };

    // Days from civil, Howard Hinnant's algorithm.
    let y = if month <= 2 { year - 1 } else { year };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let mp = (month + 9) % 12;
    let doy = (153 * mp + 2) / 5 + day - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    Some(days * 86_400 + hour * 3600 + minute * 60 + second - offset)
}

/// "November 8, 2026" from the date part of an RFC 3339 timestamp, in the timestamp's own offset.
pub fn format_date(rfc3339: &str) -> String {
    const MONTHS: [&str; 12] = [
        "January",
        "February",
        "March",
        "April",
        "May",
        "June",
        "July",
        "August",
        "September",
        "October",
        "November",
        "December",
    ];
    let parts = (|| {
        let year = rfc3339.get(0..4)?.parse::<u32>().ok()?;
        let month = rfc3339.get(5..7)?.parse::<usize>().ok()?;
        let day = rfc3339.get(8..10)?.parse::<u32>().ok()?;
        Some((MONTHS.get(month.checked_sub(1)?)?, day, year))
    })();
    match parts {
        Some((month, day, year)) => format!("{month} {day}, {year}"),
        None => "—".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::filter::StatusFilter;

    fn issue(id: &str, kind: Kind) -> Issue {
        Issue {
            id: id.repeat(8),
            human_id: id.repeat(7),
            title: id.into(),
            status: Status::Open,
            kind: Some(kind),
            epic: None,
            state: None,
            layer: None,
            labels: vec![],
            author: "me".into(),
            created: "2026-01-01T00:00:00Z".into(),
            edited: "2026-01-01T00:00:00Z".into(),
            comment_count: 1,
        }
    }

    fn child(id: &str, epic: &Issue, layer: Option<u32>) -> Issue {
        Issue {
            epic: Some(epic.id.clone()),
            layer,
            ..issue(id, Kind::Task)
        }
    }

    fn one(issues: &[Issue]) -> Vec<(&Path, &[Issue])> {
        vec![(Path::new("/r/a"), issues)]
    }

    fn all() -> Filter {
        Filter::default()
    }

    #[test]
    fn counts_ratio_and_states_come_from_children() {
        let epic = issue("a", Kind::Epic);
        let mut done = child("b", &epic, Some(1));
        done.status = Status::Closed;
        let mut busy = child("c", &epic, Some(2));
        busy.state = Some(State::InProgress);
        let mut stuck = child("d", &epic, Some(3));
        stuck.state = Some(State::Blocked);
        let mut closed_stuck = child("e", &epic, Some(4));
        closed_stuck.status = Status::Closed;
        closed_stuck.state = Some(State::Blocked);
        let issues = [epic, done, busy, stuck, closed_stuck];

        let summaries = summarize(&one(&issues), &all());
        assert_eq!(summaries.len(), 1);
        let s = &summaries[0];
        assert_eq!((s.total, s.closed), (4, 2));
        assert_eq!((s.in_progress, s.blocked), (1, 1), "closed issues skipped");
        assert_eq!(s.ratio(), 0.5);
    }

    #[test]
    fn epic_without_children_has_zero_ratio() {
        let issues = [issue("a", Kind::Epic)];
        let s = &summarize(&one(&issues), &all())[0];
        assert_eq!((s.total, s.closed), (0, 0));
        assert_eq!(s.ratio(), 0.0);
        assert!(s.children.is_empty());
        assert_eq!(s.last_activity, "2026-01-01T00:00:00Z");
    }

    #[test]
    fn children_are_ordered_by_layer_with_unlayered_last() {
        let epic = issue("a", Kind::Epic);
        let issues = [
            child("b", &epic, None),
            child("c", &epic, Some(2)),
            child("d", &epic, Some(1)),
            epic,
        ];
        let s = &summarize(&one(&issues), &all())[0];
        let titles: Vec<_> = s
            .children
            .iter()
            .map(|&c| issues[c].title.as_str())
            .collect();
        assert_eq!(titles, ["d", "c", "b"]);
    }

    #[test]
    fn children_of_another_repo_are_never_mixed_in() {
        let epic = issue("a", Kind::Epic);
        let kid = child("b", &epic, Some(1));
        let mut other_kid = child("c", &epic, Some(1));
        other_kid.status = Status::Closed;
        let first = [epic.clone(), kid];
        let second = [epic, other_kid];
        let repos: Vec<(&Path, &[Issue])> =
            vec![(Path::new("/r/a"), &first), (Path::new("/r/b"), &second)];

        let summaries = summarize(&repos, &all());
        assert_eq!(summaries.len(), 2);
        for s in &summaries {
            assert_eq!(s.total, 1);
            assert_eq!(s.children.len(), 1);
        }
        let by_repo = |r: usize| summaries.iter().find(|s| s.repo == r).unwrap();
        assert_eq!(by_repo(0).closed, 0);
        assert_eq!(by_repo(1).closed, 1);
    }

    #[test]
    fn child_pointing_at_a_missing_epic_is_ignored() {
        let ghost = issue("z", Kind::Epic);
        let issues = [issue("a", Kind::Epic), child("b", &ghost, Some(1))];
        let s = &summarize(&one(&issues), &all())[0];
        assert_eq!(s.total, 0);
    }

    #[test]
    fn last_activity_is_the_newest_instant_across_offsets() {
        let mut epic = issue("a", Kind::Epic);
        epic.edited = "2026-11-08T10:00:00-03:00".into();
        let mut early = child("b", &epic, Some(1));
        // 12:00 at +00:00 is earlier than 10:00 at -03:00 (13:00 UTC), though it sorts later as text.
        early.edited = "2026-11-08T12:00:00+00:00".into();
        let mut late = child("c", &epic, Some(2));
        late.edited = "2026-11-08T13:30:00Z".into();
        let issues = [epic, early, late];
        let s = &summarize(&one(&issues), &all())[0];
        assert_eq!(s.last_activity, "2026-11-08T13:30:00Z");
    }

    #[test]
    fn filter_applies_to_epic_status_and_repo() {
        let open = issue("a", Kind::Epic);
        let mut closed = issue("b", Kind::Epic);
        closed.status = Status::Closed;
        let issues = [open, closed];

        let titles = |filter: &Filter| -> Vec<String> {
            summarize(&one(&issues), filter)
                .iter()
                .map(|s| issues[s.epic].title.clone())
                .collect()
        };
        assert_eq!(titles(&all()), ["a"]);
        let mut f = all();
        f.status = StatusFilter::Closed;
        assert_eq!(titles(&f), ["b"]);
        f.status = StatusFilter::All;
        assert_eq!(titles(&f).len(), 2);
        f.repos.insert("/r/other".into());
        assert!(titles(&f).is_empty());
    }

    #[test]
    fn most_recently_active_epic_comes_first() {
        let mut old = issue("a", Kind::Epic);
        old.edited = "2026-01-01T00:00:00Z".into();
        let mut new = issue("b", Kind::Epic);
        new.edited = "2026-06-01T00:00:00Z".into();
        let issues = [old, new];
        let order: Vec<_> = summarize(&one(&issues), &all())
            .iter()
            .map(|s| issues[s.epic].title.clone())
            .collect();
        assert_eq!(order, ["b", "a"]);
    }

    #[test]
    fn open_counts_cover_open_issues_only() {
        let mut busy = issue("a", Kind::Task);
        busy.state = Some(State::InProgress);
        let mut stuck = issue("b", Kind::Bug);
        stuck.state = Some(State::Blocked);
        let mut closed = issue("c", Kind::Task);
        closed.status = Status::Closed;
        closed.state = Some(State::InProgress);
        let issues = [issue("d", Kind::Epic), busy, stuck, closed];
        let counts = count_open(&issues);
        assert_eq!(
            counts,
            RepoCounts {
                epics: 1,
                tasks: 1,
                bugs: 1,
                in_progress: 1,
                blocked: 1
            }
        );
        assert_eq!(counts.open(), 3);
    }

    #[test]
    fn epoch_secs_handles_offsets_fractions_and_garbage() {
        assert_eq!(epoch_secs("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(epoch_secs("1970-01-02T00:00:00+01:00"), Some(82_800));
        assert_eq!(epoch_secs("1970-01-01T00:00:00.123456-03:00"), Some(10_800));
        assert_eq!(epoch_secs("2026-11-08T18:19:03-03:00"), Some(1_794_172_743));
        assert_eq!(epoch_secs(""), None);
        assert_eq!(epoch_secs("yesterday"), None);
        assert_eq!(epoch_secs("2026-13-01T00:00:00Z"), None);
        assert_eq!(epoch_secs("2026-11-08T18:19:03"), None);
    }

    #[test]
    fn format_date_uses_the_local_date_part() {
        assert_eq!(format_date("2026-11-08T23:59:00-03:00"), "November 8, 2026");
        assert_eq!(format_date("2026-05-31T00:00:00Z"), "May 31, 2026");
        assert_eq!(format_date(""), "—");
        assert_eq!(format_date("2026-13-01T00:00:00Z"), "—");
    }

    #[test]
    fn children_and_parent_lookups() {
        let epic = issue("a", Kind::Epic);
        let late = child("b", &epic, None);
        let first = child("c", &epic, Some(1));
        let stray = issue("d", Kind::Task);
        let issues = [epic.clone(), late, first.clone(), stray.clone()];

        assert_eq!(children_of(&issues, &epic), [2, 1]);
        assert!(children_of(&issues, &first).is_empty());
        assert_eq!(parent_of(&issues, &first), Some(&epic));
        assert_eq!(parent_of(&issues, &stray), None);
        assert_eq!(parent_of(&issues, &epic), None);
    }
}
