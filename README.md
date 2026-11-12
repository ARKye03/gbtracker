# GBTracker

GBTracker is a desktop app for the issues [git-bug](https://github.com/git-bug/git-bug) stores inside your repositories. Point it at your project folders and it shows every bug, task and epic across all of them in one window, so you stop running `git bug` in each repo to find out what is open.

It reads and writes through the `git-bug` CLI, so nothing lives in a separate database. Close an issue here and `git bug` sees it closed.

![GBTracker issues view](.github/assets/preview.avif)

## Requirements

- `git` and `git-bug` installed. GBTracker looks on your `PATH`, then in `/opt/homebrew/bin`, `/usr/local/bin`, `~/go/bin`, `~/.local/bin` and `~/.nix-profile/bin`, which covers launching it from Finder or the Dock.
- A Rust toolchain with edition 2024 support.
- Network access on a clean build: `lucide-iced` downloads its icons at build time.

## Run

```sh
cargo run
GBTRACKER_THEME=dark cargo run   # force dark mode for this run
```

To build the macOS app bundle, use [`cargo-bundle`](https://github.com/burtonageo/cargo-bundle):

```sh
cargo bundle --release
```

## What you get

Add a folder on the Repositories page, either as a single project or as a parent to scan for repos (four levels deep; `node_modules` and `target` are skipped). Then:

- **Overview** gives each repo a card with its open epics, tasks and bugs, and how many are in progress or blocked.
- **Issues** lists every issue across your repos, with search, filters and an **Active work** view.
- **Epics** group child issues under their epic and show progress as closed out of total, plus how many are in progress or blocked.
- **New issue** creates a bug, task or epic, optionally under an existing epic.
- **Check Done** finds open issues whose ID already appears in a `Refs:` commit trailer on the base branch, so you can close work that has shipped. It never fetches: a stale `origin/<base>` hides finished work but never invents it.

Issue lists are cached on disk and reloaded only when a repo's git-bug refs change, so reopening the app is fast even with many repos.

## Labels

GBTracker uses plain git-bug labels, the same convention as the roadmap plugin:

| Label | Meaning |
|---|---|
| `kind:bug`, `kind:task`, `kind:epic` | Issue type |
| `epic:<id>` | Parent epic |
| `state:in-progress`, `state:blocked` | Work state on an open issue |

Check Done reads the base branch from `baseBranch` in `.claude/roadmap.json`. Without it, it falls back to `origin/HEAD`, then to the first of `main`, `trunk`, `master` or `develop` that exists.

## Keyboard

Press `?` in the app for the full list. The ones worth learning first, with `Cmd` on macOS and `Ctrl` elsewhere:

| Keys | Action |
|---|---|
| `Cmd+1`–`Cmd+4` | Overview, Issues, Epics, Repositories |
| `Cmd+K` | Search issues |
| `Cmd+N` | New issue |
| `Cmd+R` | Rescan repositories |
| `J` / `K`, `Enter` | Move through issues, open one |
| `X` | Close or reopen the selected issue |

## License

MIT. See [LICENSE](LICENSE).
