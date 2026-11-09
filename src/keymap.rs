//! Pure key-to-action resolution. The help overlay and tooltip hints render the same table, so they cannot drift from what fires.

use iced::keyboard::{Key, Modifiers, key::Named};

use crate::Section;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Action {
    Goto(Section),
    ToggleSidebar,
    FocusSearch,
    CycleTheme,
    Rescan,
    NewIssue,
    Next,
    Previous,
    Open,
    ToggleStatus,
    Help,
    Escape,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Group {
    Navigation,
    Issues,
    General,
}

impl Group {
    pub const ALL: [Group; 3] = [Group::Navigation, Group::Issues, Group::General];

    pub fn title(self) -> &'static str {
        match self {
            Group::Navigation => "Navigation",
            Group::Issues => "Issues list",
            Group::General => "General",
        }
    }
}

impl Action {
    pub fn label(self) -> String {
        match self {
            Action::Goto(section) => format!("Go to {}", section.meta().1),
            Action::ToggleSidebar => "Toggle sidebar".into(),
            Action::FocusSearch => "Search issues".into(),
            Action::CycleTheme => "Cycle theme".into(),
            Action::Rescan => "Rescan repositories".into(),
            Action::NewIssue => "New issue".into(),
            Action::Next => "Next issue".into(),
            Action::Previous => "Previous issue".into(),
            Action::Open => "Open issue details".into(),
            Action::ToggleStatus => "Close or reopen issue".into(),
            Action::Help => "Show keyboard shortcuts".into(),
            Action::Escape => "Close shortcuts or menu, then details".into(),
        }
    }

    pub fn group(self) -> Group {
        match self {
            Action::Goto(_) | Action::ToggleSidebar | Action::FocusSearch => Group::Navigation,
            Action::Next | Action::Previous | Action::Open | Action::ToggleStatus => Group::Issues,
            Action::CycleTheme
            | Action::Rescan
            | Action::NewIssue
            | Action::Help
            | Action::Escape => Group::General,
        }
    }
}

/// What the app knows that decides whether a press means anything.
#[derive(Debug, Clone, Copy)]
pub struct Context {
    /// A widget captured the press, so the user is typing.
    pub typing: bool,
    pub section: Section,
    /// Help, the Check Done sheet or the new issue dialog is up; only help and escape resolve.
    pub modal_open: bool,
}

#[derive(Debug, Clone, Copy)]
enum Pressed {
    Char(&'static str),
    Named(Named),
}

#[derive(Debug, Clone, Copy)]
struct Chord {
    key: Pressed,
    command: bool,
    shift: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Scope {
    Anywhere,
    Issues,
}

#[derive(Debug, Clone, Copy)]
struct Binding {
    chord: Chord,
    scope: Scope,
    action: Action,
}

const fn cmd(key: &'static str, action: Action) -> Binding {
    Binding {
        chord: Chord {
            key: Pressed::Char(key),
            command: true,
            shift: false,
        },
        scope: Scope::Anywhere,
        action,
    }
}

const fn cmd_shift(key: &'static str, action: Action) -> Binding {
    Binding {
        chord: Chord {
            key: Pressed::Char(key),
            command: true,
            shift: true,
        },
        scope: Scope::Anywhere,
        action,
    }
}

const fn plain(key: &'static str, scope: Scope, action: Action) -> Binding {
    Binding {
        chord: Chord {
            key: Pressed::Char(key),
            command: false,
            shift: false,
        },
        scope,
        action,
    }
}

const fn named(key: Named, scope: Scope, action: Action) -> Binding {
    Binding {
        chord: Chord {
            key: Pressed::Named(key),
            command: false,
            shift: false,
        },
        scope,
        action,
    }
}

const BINDINGS: [Binding; 19] = [
    cmd("1", Action::Goto(Section::ALL[0])),
    cmd("2", Action::Goto(Section::ALL[1])),
    cmd("3", Action::Goto(Section::ALL[2])),
    cmd("4", Action::Goto(Section::ALL[3])),
    cmd("b", Action::ToggleSidebar),
    cmd("k", Action::FocusSearch),
    cmd("f", Action::FocusSearch),
    plain("j", Scope::Issues, Action::Next),
    named(Named::ArrowDown, Scope::Issues, Action::Next),
    plain("k", Scope::Issues, Action::Previous),
    named(Named::ArrowUp, Scope::Issues, Action::Previous),
    named(Named::Enter, Scope::Issues, Action::Open),
    plain("x", Scope::Issues, Action::ToggleStatus),
    cmd_shift("l", Action::CycleTheme),
    cmd("r", Action::Rescan),
    cmd("n", Action::NewIssue),
    plain("?", Scope::Anywhere, Action::Help),
    cmd("/", Action::Help),
    named(Named::Escape, Scope::Anywhere, Action::Escape),
];

impl Chord {
    fn matches(self, key: &Key, mods: Modifiers) -> bool {
        let pressed = match (self.key, key) {
            (Pressed::Char(wanted), Key::Character(got)) => got.eq_ignore_ascii_case(wanted),
            (Pressed::Named(wanted), Key::Named(got)) => wanted == *got,
            _ => false,
        };
        if !pressed || mods.alt() {
            return false;
        }
        if self.command {
            mods.command() && mods.shift() == self.shift
        } else {
            !mods.command() && !mods.control() && !mods.logo()
        }
    }

    fn render(self, mac: bool) -> String {
        let key = match self.key {
            Pressed::Char(c) => c.to_uppercase(),
            Pressed::Named(Named::Escape) => "Esc".into(),
            Pressed::Named(Named::ArrowDown) => "↓".into(),
            Pressed::Named(Named::ArrowUp) => "↑".into(),
            Pressed::Named(Named::Enter) if mac => "↵".into(),
            Pressed::Named(Named::Enter) => "Enter".into(),
            Pressed::Named(other) => format!("{other:?}"),
        };
        match (self.command, mac) {
            (false, _) => key,
            (true, true) => format!("⌘+{}{key}", if self.shift { "⇧+" } else { "" }),
            (true, false) => format!("Ctrl+{}{key}", if self.shift { "Shift+" } else { "" }),
        }
    }
}

impl Binding {
    fn applies(&self, key: &Key, mods: Modifiers, ctx: Context) -> bool {
        if ctx.modal_open && !matches!(self.action, Action::Help | Action::Escape) {
            return false;
        }
        if self.scope == Scope::Issues && ctx.section != Section::Issues {
            return false;
        }
        let closes_modal = ctx.modal_open && self.action == Action::Escape;
        if !self.chord.command && ctx.typing && !closes_modal {
            return false;
        }
        self.chord.matches(key, mods)
    }
}

/// `key` must be the shift-applied key for plain presses (so `?` is `?`) and the unmodified key for command chords.
pub fn resolve(key: &Key, mods: Modifiers, ctx: Context) -> Option<Action> {
    BINDINGS
        .iter()
        .find(|b| b.applies(key, mods, ctx))
        .map(|b| b.action)
}

fn on_mac() -> bool {
    cfg!(target_os = "macos")
}

/// First chord bound to `action`, rendered for this platform.
pub fn hint(action: Action) -> Option<String> {
    BINDINGS
        .iter()
        .find(|b| b.action == action)
        .map(|b| b.chord.render(on_mac()))
}

/// One help row per action, in table order, with every chord that triggers it.
pub fn help_rows(group: Group) -> Vec<(String, Vec<String>)> {
    rows(group, on_mac())
}

fn rows(group: Group, mac: bool) -> Vec<(String, Vec<String>)> {
    let mut out: Vec<(Action, Vec<String>)> = Vec::new();
    for b in BINDINGS.iter().filter(|b| b.action.group() == group) {
        let chord = b.chord.render(mac);
        match out.iter_mut().find(|(a, _)| *a == b.action) {
            Some((_, chords)) => chords.push(chord),
            None => out.push((b.action, vec![chord])),
        }
    }
    out.into_iter().map(|(a, c)| (a.label(), c)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CMD: Modifiers = Modifiers::COMMAND;

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn ctx(section: Section) -> Context {
        Context {
            typing: false,
            section,
            modal_open: false,
        }
    }

    fn typing(section: Section) -> Context {
        Context {
            typing: true,
            ..ctx(section)
        }
    }

    #[test]
    fn command_digits_pick_sections_in_order() {
        for (i, section) in Section::ALL.iter().enumerate() {
            let key = ch(&(i + 1).to_string());
            assert_eq!(
                resolve(&key, CMD, ctx(Section::Epics)),
                Some(Action::Goto(*section)),
                "Cmd+{}",
                i + 1
            );
        }
    }

    #[test]
    fn command_digits_past_the_last_section_are_unbound() {
        for digit in ["5", "6", "7", "8"] {
            assert_eq!(resolve(&ch(digit), CMD, ctx(Section::Epics)), None);
        }
    }

    #[test]
    fn command_bindings_resolve() {
        let c = ctx(Section::Repositories);
        for (key, mods, action) in [
            ("b", CMD, Action::ToggleSidebar),
            ("k", CMD, Action::FocusSearch),
            ("f", CMD, Action::FocusSearch),
            ("l", CMD | Modifiers::SHIFT, Action::CycleTheme),
            ("L", CMD | Modifiers::SHIFT, Action::CycleTheme),
            ("r", CMD, Action::Rescan),
            ("n", CMD, Action::NewIssue),
            ("/", CMD, Action::Help),
        ] {
            assert_eq!(resolve(&ch(key), mods, c), Some(action), "{key}");
        }
    }

    #[test]
    fn command_bindings_ignore_wrong_shift_and_wrong_modifier() {
        let c = ctx(Section::Issues);
        assert_eq!(resolve(&ch("l"), CMD, c), None);
        assert_eq!(resolve(&ch("k"), CMD | Modifiers::SHIFT, c), None);
        assert_eq!(resolve(&ch("k"), CMD | Modifiers::ALT, c), None);
        assert_eq!(resolve(&ch("j"), CMD, c), None);
    }

    #[test]
    fn command_bindings_fire_while_typing() {
        let c = typing(Section::Issues);
        assert_eq!(resolve(&ch("k"), CMD, c), Some(Action::FocusSearch));
        assert_eq!(
            resolve(&ch("1"), CMD, c),
            Some(Action::Goto(Section::ALL[0]))
        );
    }

    #[test]
    fn single_keys_resolve_in_the_issues_list() {
        let c = ctx(Section::Issues);
        let none = Modifiers::NONE;
        assert_eq!(resolve(&ch("j"), none, c), Some(Action::Next));
        assert_eq!(resolve(&ch("k"), none, c), Some(Action::Previous));
        assert_eq!(resolve(&ch("x"), none, c), Some(Action::ToggleStatus));
        assert_eq!(
            resolve(&Key::Named(Named::ArrowDown), none, c),
            Some(Action::Next)
        );
        assert_eq!(
            resolve(&Key::Named(Named::ArrowUp), none, c),
            Some(Action::Previous)
        );
        assert_eq!(
            resolve(&Key::Named(Named::Enter), none, c),
            Some(Action::Open)
        );
    }

    #[test]
    fn typing_in_an_input_is_never_hijacked() {
        let c = typing(Section::Issues);
        let none = Modifiers::NONE;
        for key in [ch("j"), ch("k"), ch("x"), ch("?")] {
            assert_eq!(resolve(&key, none, c), None, "{key:?}");
        }
        for named in [Named::Enter, Named::Escape] {
            assert_eq!(resolve(&Key::Named(named), none, c), None, "{named:?}");
        }
    }

    #[test]
    fn list_keys_do_nothing_outside_the_issues_page() {
        let c = ctx(Section::Epics);
        assert_eq!(resolve(&ch("j"), Modifiers::NONE, c), None);
        assert_eq!(resolve(&ch("x"), Modifiers::NONE, c), None);
        assert_eq!(resolve(&Key::Named(Named::Enter), Modifiers::NONE, c), None);
    }

    #[test]
    fn question_mark_and_escape_work_anywhere() {
        let c = ctx(Section::Repositories);
        assert_eq!(
            resolve(&ch("?"), Modifiers::SHIFT, c),
            Some(Action::Help),
            "shift is already folded into the key"
        );
        assert_eq!(resolve(&ch("?"), Modifiers::NONE, c), Some(Action::Help));
        assert_eq!(
            resolve(&Key::Named(Named::Escape), Modifiers::NONE, c),
            Some(Action::Escape)
        );
    }

    #[test]
    fn open_help_swallows_everything_but_help_and_escape() {
        let c = Context {
            modal_open: true,
            ..ctx(Section::Issues)
        };
        assert_eq!(resolve(&ch("j"), Modifiers::NONE, c), None);
        assert_eq!(resolve(&ch("b"), CMD, c), None);
        assert_eq!(resolve(&ch("?"), Modifiers::NONE, c), Some(Action::Help));
        assert_eq!(resolve(&ch("/"), CMD, c), Some(Action::Help));
        assert_eq!(
            resolve(&Key::Named(Named::Escape), Modifiers::NONE, c),
            Some(Action::Escape)
        );
    }

    #[test]
    fn every_table_entry_resolves_to_its_own_action() {
        for b in BINDINGS {
            let key = match b.chord.key {
                Pressed::Char(c) => ch(c),
                Pressed::Named(n) => Key::Named(n),
            };
            let mut mods = if b.chord.command {
                CMD
            } else {
                Modifiers::NONE
            };
            if b.chord.shift {
                mods |= Modifiers::SHIFT;
            }
            let first = BINDINGS
                .iter()
                .find(|o| o.chord.matches(&key, mods) && o.scope == b.scope)
                .unwrap();
            assert_eq!(
                resolve(&key, mods, ctx(Section::Issues)),
                Some(first.action)
            );
        }
    }

    #[test]
    fn chords_render_per_platform() {
        let theme = Chord {
            key: Pressed::Char("l"),
            command: true,
            shift: true,
        };
        assert_eq!(theme.render(true), "⌘+⇧+L");
        assert_eq!(theme.render(false), "Ctrl+Shift+L");
        let search = Chord {
            key: Pressed::Char("k"),
            command: true,
            shift: false,
        };
        assert_eq!(search.render(true), "⌘+K");
        assert_eq!(search.render(false), "Ctrl+K");
        let enter = Chord {
            key: Pressed::Named(Named::Enter),
            command: false,
            shift: false,
        };
        assert_eq!(enter.render(true), "↵");
        assert_eq!(enter.render(false), "Enter");
    }

    #[test]
    fn help_lists_every_action_once_with_all_its_chords() {
        let all: Vec<_> = Group::ALL.iter().flat_map(|g| rows(*g, false)).collect();
        assert_eq!(all.len(), 4 + 2 + 4 + 5);
        let search = all.iter().find(|(l, _)| l == "Search issues").unwrap();
        assert_eq!(search.1, ["Ctrl+K", "Ctrl+F"]);
        let help = all
            .iter()
            .find(|(l, _)| l == "Show keyboard shortcuts")
            .unwrap();
        assert_eq!(help.1, ["?", "Ctrl+/"]);
        let sections = rows(Group::Navigation, true);
        assert_eq!(sections[0], ("Go to Overview".into(), vec!["⌘+1".into()]));
    }

    #[test]
    fn hints_use_the_first_chord() {
        assert_eq!(
            hint(Action::FocusSearch),
            Some(
                Chord {
                    key: Pressed::Char("k"),
                    command: true,
                    shift: false
                }
                .render(on_mac())
            )
        );
        assert!(hint(Action::Goto(Section::Repositories)).is_some());
    }
}
