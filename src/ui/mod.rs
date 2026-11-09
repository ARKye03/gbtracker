pub mod components;
pub mod detail;
pub mod done;
pub mod screen;
pub mod tooltip;

pub fn is_user_label(label: &str) -> bool {
    !["kind:", "epic:", "state:"]
        .iter()
        .any(|p| label.starts_with(p))
}
