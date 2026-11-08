//! Static sample data for the UI mock. Shapes mirror what git-bug exposes, so the real app can swap these for queries.

pub struct Epic {
    pub id: &'static str,
    pub title: &'static str,
    pub repo: &'static str,
    pub total: u32,
    pub closed: u32,
    pub opened: &'static str,
    pub last_activity: &'static str,
    pub labels: &'static [&'static str],
}

impl Epic {
    pub fn ratio(&self) -> f32 {
        self.closed as f32 / self.total.max(1) as f32
    }
}

pub const IDENTITY_INITIALS: &str = "RS";

pub const EPICS: &[Epic] = &[
    Epic {
        id: "a3f9c21",
        title: "Sync engine rewrite",
        repo: "~/personal-space/gbtracker",
        total: 24,
        closed: 15,
        opened: "May 31, 2026",
        last_activity: "November 6, 2026",
        labels: &["kind:epic", "area:sync", "priority:high", "L1–L6"],
    },
    Epic {
        id: "7be04d1",
        title: "GitHub bridge import",
        repo: "~/code/bridge-kit",
        total: 18,
        closed: 5,
        opened: "July 14, 2026",
        last_activity: "November 2, 2026",
        labels: &["kind:epic", "bridge:github"],
    },
    Epic {
        id: "c19e8a0",
        title: "Onboarding polish",
        repo: "~/personal-space/gbtracker",
        total: 9,
        closed: 7,
        opened: "September 3, 2026",
        last_activity: "November 8, 2026",
        labels: &["kind:epic", "area:ui", "good-first-issue"],
    },
    Epic {
        id: "04d7f3e",
        title: "Legacy label cleanup",
        repo: "~/code/old-monorepo",
        total: 31,
        closed: 3,
        opened: "March 2, 2026",
        last_activity: "August 19, 2026",
        labels: &["kind:epic", "chore", "state:blocked"],
    },
];
