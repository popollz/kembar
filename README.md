# kembar

Terminal duplicate finder for macOS. Review identical files and move the redundant copies to Trash.

It works in the spirit of [Gemini 2](https://gemini.macpaw.com) and lives in the shell like [yazi](https://github.com/sxyazi/yazi).

```
 kembar  /Users/you/Pictures                                       412 groups · 38.2 GB freeable
┌ Groups (412) ────────────────────────┐┌ Copies (3) · 12.4 MB each ───────────────────────┐
│   12.4 MB    • 3×  IMG_0421.HEIC     ││ [x]  IMG_0421.HEIC        12.4 MB    3d ago      │
│    4.1 GB    • 2×  vacation.mov      ││ [ ]★ IMG_0421.HEIC        12.4 MB    2y ago      │
│  880.0 KB     2×  report-final.pdf   ││ [x]  IMG_0421 copy.HEIC   12.4 MB    1d ago      │
└──────────────────────────────────────┘└──────────────────────────────────────────────────┘
┌ Details ────────────────────────────────────────────────────────────────────────────────┐
│ /Users/you/Pictures/2024/IMG_0421.HEIC                                                   │
│ 12.4 MB each · 3 copies · 24.8 MB freeable                                               │
│ blake3 9f2c1b8e4a7d…                                                                      │
│ keep oldest → /Users/you/Pictures/2024/IMG_0421.HEIC                                     │
└──────────────────────────────────────────────────────────────────────────────────────────┘
 j/k move  Tab pane  space mark  a select  s keep  d trash  D purge  o finder  r rescan  / filter  ? help  q quit
```

## Why this exists

Gemini's loop is: scan → group duplicates → **pre-select what to delete** → review → clean. The terminal
version keeps that loop but makes it scriptable and fast.

Rather than reinventing duplicate detection, `kembar` drives [`czkawka_core`](https://crates.io/crates/czkawka_core)
(the engine behind Czkawka/Krokiet, MIT licensed) and adds a yazi-style review UI, Smart Selection, and
macOS-native Trash handling on top. That means the expensive parts — parallel walking, size bucketing,
4 KB prehashing, BLAKE3 full hashes, hard-link filtering — are already battle-tested.

## Build

```sh
# Rust is installed at ~/.cargo/bin (rustup). Open a new shell or:
source "$HOME/.cargo/env"

cd ~/Documents/Purwarupa/kembar
cargo build --release
./target/release/kembar --help
```

`~/.cargo/bin` is already on PATH, so linking there makes `kembar` available everywhere:

```sh
ln -sf "$PWD/target/release/kembar" ~/.cargo/bin/kembar
```

Note: always run the **release** build. Debug builds make `czkawka_core` dump its internal
scan statistics to stdout.

## Use

```sh
kembar                          # scan the current directory, open the TUI
kembar ~/Pictures ~/Downloads   # several roots at once
kembar ~ -m 1M -x jpg,png,heic  # only photos above 1 MB
kembar ~/Documents --report     # human readable summary, no TUI
kembar ~ --json > dupes.json    # machine readable
kembar ~ --keep newest -e "*/.build/*"
kembar --init-config            # write ~/Library/Application Support/kembar/config.toml
```

Non-interactive output (a pipe) automatically falls back to the text report.

### Keys

| Key | Action |
|---|---|
| `j` / `k`, arrows | move |
| `g` / `G` | first / last |
| `Tab`, `h`, `l`, `Enter` | switch between groups and copies |
| `space` | toggle a copy (groups pane: smart-mark the whole group) |
| `a` | Smart Selection — mark every copy except the keeper |
| `A` | clear all marks |
| `s` | cycle keep strategy (`oldest` → `newest` → `shallowest` → `deepest` → `first`) |
| `d` / `D` | move marked files to Trash / permanently delete (both need `y`) |
| `o` | reveal the current file in Finder |
| `/` | filter groups by path |
| `r` | rescan |
| `?` | help |
| `q` | quit |

Smart Selection runs automatically when a scan finishes, so the common flow is: wait, skim, `d`, `y`.

### Keep strategies

`oldest` (default) keeps the earliest modified copy and penalises paths that look like copies
(`copy`, `duplicate`, `backup`, ` (1)`, ` 2.` …). `shallowest` prefers the copy closest to the scan root.
Ties break on modification time, then path depth, then path length.

## Safety model

- `d` uses `trash`, which calls the native macOS APIs: recoverable from Finder, and it works on external volumes.
- `D` asks for confirmation and is irreversible.
- Every delete writes a manifest to `~/Library/Application Support/kembar/history/` listing exact paths and timestamps.
- The app refuses to remove **every** copy of a group, even if you mark them all.
- `--json` / `--report` never delete anything.

## macOS permission gotcha

On current macOS, a terminal app can only read `~/Desktop`, `~/Documents`, `~/Downloads`, and parts of
`~/Library` if the terminal has **Full Disk Access** (System Settings → Privacy & Security → Full Disk Access).
Without it you'll see fewer files, not an error. Grant it to your terminal (Ghostty, iTerm2, Terminal, …),
then restart the terminal.

## Performance notes (M4 Pro)

- Walking + size bucketing is `jwalk`-parallel; hashing is Rayon-parallel and BLAKE3 uses ARM NEON.
- Only files that share an exact size get hashed, and only a 4 KB head/tail prehash at first.
- Hard links are filtered out (same inode ≠ wasted space).
- The SSD is usually the bottleneck (~5–6 GB/s), so a 500 GB home scan is typically a few minutes.
- `--cache` keeps czkawka's hash cache between runs; handy for repeated scans of a large photo library.

Default excludes: `.git`, `node_modules`, `.Trash`, Spotlight/FSEvents metadata, `Library/Caches`, `.app`,
`.photoslibrary`, `.fcpbundle`, `.logicx`. Add more with `-e` or `config.toml`.

## Roadmap

1. **Similar photos** — `czkawka_core::tools::similar_images` is already in the dependency; add a `--similar-images`
   mode with perceptual-hash distance and a thumbnail-free review pane.
2. **Similar videos** — frame sampling + audio fingerprint (`similar_videos` in the engine) behind a feature flag.
3. **Duplicate folders** and **empty files/folders** — same pattern, small additions.
4. **Undo** — a quarantine mode (`~/.local/share/kembar/quarantine/<run>/…`) with one-key restore.
5. **App bundles** — sign + notarise if this ever ships outside the terminal.

## License

[MIT](LICENSE). Copyright (c) 2026 popollz.

Duplicate detection is [`czkawka_core`](https://github.com/qarmin/czkawka) (MIT, Copyright 2020–2026 Rafał Mikrut). A release build also statically links `rawler` (LGPL-2.1) and `symphonia` (MPL-2.0) through that library. Those components keep their own licenses. The source in this repository is MIT.
