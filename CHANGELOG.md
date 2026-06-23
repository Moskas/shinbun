# Changelog

## [0.2.0] - 2026-06-23

### Features

- better entry scrolling
- changed how the ui renders depending on terminal width
- dynamic entry text padding
- rewrite, added basic config options
- added async feeds fetching and error popup
- initial feed cache implementation
- added option to query feeds with tags
- added read and unread display
- add sqlite to nix devshell
- moved feed loading info to a popup
- change lists to tables, fix feed title config
- added open entry in external application
- removed dead code, optimization
- Small feed list display changes
- config reorganization
- style changes in views
- go to top/bottom, prune dead feeds
- Added toggle for hiding read entires
- split view removal
- better entry render, bump deps
- added naive link list to entry view
- added option to do single refresh
- custom stylesheet for tui-markdown
- code polish
- help popup, tests
- keybind to mark whole feed as read
- updated README
- readability and style changes
- link list popup in entry view
- fuzzy search for entry and feeds list
- ui polish, added show_scrollbar config
- github actions
- fix no entries bug
- rustfmt config
- added tag list view, fixed styling when fuzzy matching
- custom theme config under ui.theme, updated README
- link yanking
- added import/export to opml
- added refresh command to CLI
- added stats and clean commands
- per-feed refresh intervals with skip indicators
- skip redundant renders when idle and eliminate unnecessary SQLite reloads
- cap feed-source column width on narrow terminals with ellipsis truncation
- avoid Vec<char> allocs and re-lowercasing pattern per candidate
- replace remove_dead_feeds SELECT+loop with single SQL DELETE
- add 30s HTTP timeout and drop redundant feed_name clone
- O(1) is_being_fetched lookup and remove extra cloned().collect()
- O(feeds+entries) in-memory sync for query mark-all-read
- render inline images in entry view via ratatui-image
- config/toggle for image rendering
- add feed from TUI with 'a' keybind

### Bug Fixes

- openssl in dev shell
- entry text not being parsed
- display and updates of read/unread status
- clippy and fmt
- bug with taglist entry view and refreshing feeds
- youtube description miss newlines
- fmt
- use feed URL instead of title to resolve query feed entries
- redraw on terminal resize and after loading linger expires
- set User-Agent header on HTTP requests to avoid being blocked

### Refactoring

- split app.rs to smaller files
- optimize feed pruning with batched transaction, add cache init error handling

### Chores

- flake update
- bump cargo deps
- Cargo.lock update
- cargo fmt and clippy fixes
- flake & cargo deps bump, clippy warning fix
- clippy & fmt
