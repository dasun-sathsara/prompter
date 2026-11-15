# Prompter

A floating script window for macOS. It stays above whatever you are reading
from, it is see-through, and it never steals focus.

Built in Rust with [GPUI](https://github.com/zed-industries/zed), the framework
behind the Zed editor.

## What it does

- Sits above every other window, including Chrome in full-screen, on every space.
- Never takes focus. Clicking the panel does not bring its app forward, and
  clicking another app does not hide it.
- Resizes from any edge or corner. Drags from the strip at the top.
- Fades its own backdrop from 15% to 100%. The words stay at full contrast, so
  the script never goes muddy.
- Sets type from 10 to 120 points in any installed font, at five weights, with a
  search field to find a family among the hundreds macOS reports, and renders
  `*italic*` and `**bold**` Markdown emphasis.
- Rolls the script on its own, at a speed you can adjust.

## Using it

| Action | How |
| --- | --- |
| Move | Drag the dotted grip or the empty part of the top strip |
| Resize | Drag any window edge or corner |
| Text size | `−` / `+` in the strip, `Command +` / `Command -`, or `Command 0` to reset |
| Backdrop | The slider in the strip, drag the knob |
| Typeface | `Aa` opens the family list, type to search, `↑` / `↓` and `Return` to pick |
| Weight | The five `Aa` chips above the family list |
| Paste a script | `Command V` |
| Open a file | `Command O`, or drop a text file onto the window |
| Roll / pause | `▸` in the strip, or `Space` (`Escape` pauses) |
| Roll speed | Click the speed number, scroll over `▸`, `[` / `]`, or `Shift ↑` / `Shift ↓` |
| Scroll by hand | Wheel, `↑` / `↓`, `PageUp` / `PageDown`, or `Home` |
| Quit | `×` in the strip, or `Command Q` |

A script can also be passed on the command line: `prompter ~/script.txt`.

Size, opacity, typeface, position and the last opened file are remembered in
`~/Library/Application Support/Prompter/settings.json`.

## Building

Needs a Rust toolchain and, on macOS, the Xcode command line tools.

```sh
cargo build --release
./scripts/bundle.sh target/release/prompter
open dist/Prompter.app
```

For an Apple silicon bundle from the release profile:

```sh
cargo build --release --target aarch64-apple-darwin
./scripts/bundle.sh
```

`scripts/bundle.sh` writes `dist/Prompter.app` and ad-hoc signs it, which is
enough to run it locally. It is not notarised, so a copy that arrives through a
download carries a quarantine flag: open it once with right click → Open.

### Building the Apple silicon bundle on Linux

This repository was built that way. Two things are needed beyond a normal
cross-link: a macOS SDK and clang with ld64.lld.

```sh
rustup target add aarch64-apple-darwin
MAC_SDK=/path/to/MacOSX14.5.sdk ./scripts/build-mac.sh
```

`scripts/cross-env.sh` holds the linker settings. If the build stops on a
missing `stitched_shaders.metal`, GPUI's macOS crate skipped its shader step
because that build script is gated on the host OS. Run
`MAC_SDK=... ./scripts/stitch-shaders.sh` once, which generates that file and
reruns the build. The output is a Mach-O arm64 binary, ad-hoc signed by the
linker, which is what Apple silicon requires to run an unsigned build at all.

## How the window is put together

There is no window title, in the strip or in the window menu.

The window is a `WindowKind::PopUp` in GPUI, which on macOS is an `NSPanel` with
the non-activating style at `NSPopUpWindowLevel`, plus
`CanJoinAllSpaces | FullScreenAuxiliary`. The app runs as an accessory app, so
it has no Dock icon and no menu bar. `src/mac.rs` adds the parts GPUI does not
set: `setHidesOnDeactivate(false)` so the panel survives another app taking
focus, `mouseDownCanMoveWindow` overridden to `NO` and `app_owns_titlebar_drag: true`
so AppKit never steals slider drags in the titlebar strip, hidden traffic lights,
no AppKit shadow, and stationary collection behaviour.

`PROMPTER_DEBUG_WINDOW=1 ./Prompter.app/Contents/MacOS/prompter` prints what
AppKit holds for the window, which is the quickest way to check the panel is
still non-activating, resizable and at pop-up level.

The transparency is per-pixel alpha from `WindowBackgroundAppearance::Transparent`.
The panel's fill colour carries the opacity; the text colour does not, which is
why the script stays legible at 15%.
