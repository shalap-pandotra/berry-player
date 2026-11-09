<img src="assets/berry_readme.png" width="128" align="left" alt="berry">

# berry

A small, fast music player for the terminal, with album art.

berry plays your local music library inside Windows Terminal: browse by album or artist, search as you type, build a queue, and see real album covers right in the terminal. It's a single executable of a few megabytes with no dependencies to install, and it remembers your library between launches so it opens almost instantly.

<br clear="left">

## Features

- Album covers drawn in the terminal (via Sixel), previewed as you browse
- Albums, artists, and search tabs, plus a queue you can reorder
- Live search across artists, albums, and songs
- Sorting by title, artist, year, or recently added, remembered between launches
- Follows your headphones: switch audio devices and playback moves over on its own
- Plays MP3, FLAC, OGG, M4A/AAC, WAV, and AIFF
- Keyboard driven, with Vim-style keys
- Uses your terminal's colors, so it matches whatever theme you run

## Requirements

- Windows 10 or 11
- **Windows Terminal 1.22 or newer.** That's the version that added Sixel image support; on older versions berry still works, just without covers.
- That's it. berry works with any font. If your terminal uses a [Nerd Font](https://www.nerdfonts.com/), you can swap the playback controls for its nicer icons by setting `BERRY_ICONS=nerd` (see below).

## Install

**The easy way:** download `berry.exe` from the [latest release](../../releases/latest) and put it somewhere on your PATH, or anywhere you like and run it from there.

Windows may show a SmartScreen warning the first time, since the exe isn't signed. Choose "More info" and then "Run anyway".

**From source:** install [Rust](https://rustup.rs) (including the Visual Studio C++ Build Tools it offers during setup), then:

```
cargo install --git https://github.com/shalap-pandotra/berry-player
```

## Usage

```
berry                      # plays from your Music folder
berry "D:\Music"           # or any folder you point it at
```

berry scans the folder (subfolders included) and groups songs into albums by their tags. Songs without tags still show up, named after the file and grouped by folder. Covers come from art embedded in the files, or a `cover.jpg` / `folder.jpg` next to them.

The first launch reads every file; after that berry only rereads files that are new or changed, so it opens fast. The cache lives at `%LOCALAPPDATA%\berry\library.db`, and it's safe to delete if you ever want a fresh scan.

## Screenshots

<p align="center"><img src="assets/screenshots/playing.png" width="512" alt="playing">
</p>

<p align="center"><img src="assets/screenshots/browsing.png" width="512" alt="playing"></p>
<p align="center"><img src="assets/screenshots/search.png" width="512" alt="playing"></p>

the screenshots above come from a my terminal which uses a custom theme. if you run berry in just a stock windows terminal, this is what it looks like. it just picks up whatever colors your terminal uses.

<p align="center"><img src="assets/screenshots/stock.png" width="512" alt="playing"></p>

## Keys

Press `?` inside berry to see these at any time.

| Key | Does |
| --- | --- |
| `enter` | play now |
| `a` / `A` | add to queue / add the whole album or artist |
| `space` | pause / resume |
| `n` / `p` | next / previous |
| `,` / `.` | seek back / forward 5 seconds |
| `-` / `=` | volume down / up |
| `1` to `4` | jump to a tab |
| `tab` / `shift+tab` | next / previous tab |
| `` ` `` | back to the last tab |
| `j` / `k` | down / up |
| `h` / `l` | left / right column |
| `/` | search |
| `o` | sort the list you're on |
| `J` / `K` | move a song in the queue |
| `d` / `D` | remove a song / clear the queue |
| `q` | quit |

## Settings

berry remembers your sort choices on its own. One more option, set before launching:

```
$env:BERRY_ICONS="nerd"; berry     # Nerd Font icons for the playback controls
```

To make it stick, add `$env:BERRY_ICONS="nerd"` to your PowerShell profile, or set it as a user environment variable.

## Troubleshooting

If berry closes unexpectedly, run it with `BERRY_DEBUG` set to see the error:

```
$env:BERRY_DEBUG=1; berry
```
