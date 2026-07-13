# ncoxide

> **Pre-release software.** ncoxide is in early development. Expect breaking
> changes, missing features, and rough edges. Use at your own risk.

A modal dual-pane file commander for the terminal, inspired by
[Helix](https://helix-editor.com/).

## Features

- **Dual-pane layout** -- two independent file panels side by side; copy, move,
  and compare files across panes
- **Modal keybindings** -- Normal, Select, Space menu, Goto, Command, Input, and
  Finder modes (similar to Helix/Vim)
- **Fuzzy finder** -- powered by nucleo-matcher (the same engine Helix uses)
- **Syntax-highlighted preview** -- inline file preview with syntect for source
  files; large files and logs stream through a windowed reader with bounded
  memory (scrolls multi-GB files without loading them)
- **File viewer** -- full-screen pager (Space then `v`) with in-file search
  (`/`, literal or `Ctrl-R` regex, `n`/`N` to cycle matches), goto-line
  (`<N>G`), and fuzzy line-filter (`&`, powered by nucleo-matcher)
- **Visual selection** -- select individual files, ranges, or by glob pattern
  (`*.rs`)
- **File operations** -- copy, move, delete, rename, mkdir with confirmation
  dialogs
- **Goto shortcuts** -- jump to home, root, other pane, previous directory, or
  numbered bookmarks
- **Sorting & filtering** -- sort by name, size, date, or extension; filter by
  substring; toggle hidden files
- **Open in `$EDITOR`** -- press `Space` then `e` to edit the file under the
  cursor (or `Enter` on a file)
- **Command mode** -- `:cd`, `:sort`, and `:q` commands

## Installation

Requires **Rust 1.85+** (edition 2024).

```
cargo install ncoxide
```

## Usage

```
ncoxide [OPTIONS]

Options:
  -l, --left <DIR>     Left pane starting directory
  -r, --right <DIR>    Right pane starting directory
      --log <FILE>     Log file path [default: /tmp/ncoxide.log]
  -h, --help           Print help
  -V, --version        Print version
```

Both panes default to the current working directory.

```bash
# Open with default directories
ncoxide

# Open specific directories in each pane
ncoxide --left ~/projects --right /tmp
```

## Quick reference

| Key | Action |
|-----|--------|
| `j` / `k` | Move cursor down / up |
| `h` | Parent directory |
| `l` / `Enter` | Enter directory or open file |
| `Tab` | Switch active pane |
| `p` | Toggle preview pane |
| `v` | Enter select mode |
| `;` | Clear selection (selections persist across modes) |
| `Space` | Open action menu |
| `g` | Goto mode (`gg` top, `G` bottom, `gh` home, `gr` root) |
| `Space` `f` | Fuzzy finder |
| `/` | Filter files |
| `.` | Toggle hidden files |
| `y` | Copy to other pane |
| `Space` `m` | Move to other pane |
| `d` | Delete (with confirmation) |
| `r` | Rename |
| `Space` `e` | Edit in `$EDITOR` |
| `Space` `v` | View file (pager) |
| `?` | Help screen |
| `q` | Quit |

## Project status

ncoxide is a personal project in early development. The core features listed
above are implemented and working, but the API and keybindings may change at any
time. Bug reports and feedback are welcome on the
[issue tracker](https://todo.sr.ht/~danprobst/ncoxide).

## License

MIT -- see [LICENSE](LICENSE) for details.
