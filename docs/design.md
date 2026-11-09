# gbtracker UI design

Desktop app (Rust, iced 0.14) that wraps directories using git-bug. Visual target: `docs/proposal.jpg`. Repositories, Issues, Epics and Overview read real git-bug data; the other sections are placeholders.

## Run

```sh
cargo run                         # follows OS theme (or cycle System/Light/Dark with the top bar button)
GBTRACKER_THEME=dark cargo run    # force dark for this run
```

## Code map

| Path | Role |
|------|------|
| `src/theme/mod.rs` | `Mode` (Light/Dark), `Tokens` (all colors), `Tone` (success/warning/danger) |
| `src/theme/style.rs` | Style fns for `.style(...)`, plus `radius::*` |
| `src/ui/components.rs` | Building blocks: pills, buttons, chip, dot, detail row, split progress |
| `src/filter.rs` | `Filter`, pure `matches`, `/roadmap-open` ordering (`roadmap_order`) |
| `src/ui/screen.rs` | Screens: top bar, sidebar, Repositories, Issues, Epics, Overview |
| `src/epics.rs` | `EpicSummary` (children, ratio, in-progress/blocked, last activity) and per-repo open counts |

## Theme rules

- Colors come only from `Tokens`. Style fns call `Tokens::of(theme)`, which picks `LIGHT` or `DARK` based on `is_dark`. No hex values in screens.
- Surfaces, from back to front: `app_bg` (window + top bar) → `panel` (content area, rounded top) → `surface` (cards, active nav item).
- Neutral fills: `fill` for secondary buttons and pills, `chip` for labels and icon squares.
- One accent color, mauve: `#8839ef` in light mode, `#cba6f7` in dark mode. Text on the dark accent is `#1e1e2e` because white is unreadable on it. Use the accent for the single primary action per page. The proposal's green still marks progress and status (`Tone`).
- `Tone::for_ratio`: ≥50% success, ≥20% warning, otherwise danger. Bars use the strong tone on a tinted track; the remaining-percentage label uses the soft tone.
- Radii: `SM` 6 for chips, `MD` 8 for buttons, `LG` 12 for pills and nav items, `XL` 18 for cards and the panel.
- Selected state is never just a surface change. Chips and the active top pill use `chip_selected`: `accent_soft` fill, 1px `accent` border, `accent` text (≥3:1 against the panel, unit-tested for both palettes). Unselected chips are transparent with a 1px `border` and `text_muted`. Multi-select groups (Kind) add a leading check. The active sidebar item adds a 3px accent bar.
- Card pages use `components::card_grid`: columns = `(w + gap) / (min_width + gap)`, cards are `Fill` width, the last row is padded to stay aligned. No fixed-width cards.
- Shadows are a single soft 1px/6 blur, only on raised white surfaces.
- Type scale: 29 for card headline numbers, 21 for page and sidebar titles, 18 for card titles, 15 for controls, 14 for body text, 13 for meta, chips and percentages.

## How the proposal maps to git-bug

| Proposal | gbtracker |
|----------|-----------|
| Org Settings tab | Active repo tab (`gbtracker`) |
| Job Postings tab | Repositories, the list of wrapped dirs |
| Talk to AI box | Sync status pill (dot uses `Tone`) |
| Sidebar sections | Overview, Issues, Epics, Labels, Identities, Bridges, Analytics |
| Budget card | Epic card: bug count, title, closed/open bar, details, labels, short id |
| Add Budget | New Epic |

## Known gaps / next steps

- **Dashed borders:** iced has none, so "Add Filters" uses a solid 1px `border`.
- **Font:** uses the system sans. The proposal looks like Inter; to match it, bundle Inter with `.font(...)` and set `.default_font(...)`.
- **Icons:** `lucide-iced` downloads Lucide at build time, so clean builds need network access.
