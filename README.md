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
- Fades its own backdrop from 10% to 100%. The words stay at full contrast so the
  text never goes muddy.
- Sets type from 16 to 120 points in any installed font, including the fonts you
  have added to macOS.
- Rolls the script on its own, at a speed you can nudge.

## Using it

| Action | How |
| --- | --- |
| Move | Drag the dotted grip or the empty part of the top strip |
| Resize | Drag any window edge or corner |
| Text size | `−` / `+` in the strip, or `Command +` / `Command -` |
| Backdrop | The slider in the strip, drag the knob |
| Typeface | `Aa` opens the family list |
| Paste a script | `Command V` |
| Open a file | `Command O` |
| Roll the script | `Space` |
| Scroll by hand | Wheel, or `↑` / `↓` |
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

## How the window is put together

The window is a `WindowKind::PopUp` in GPUI, which on macOS is an `NSPanel` with
the non-activating style at `NSPopUpWindowLevel`, plus
`CanJoinAllSpaces | FullScreenAuxiliary`. The app runs as an accessory app, so
it has no Dock icon and no menu bar. `src/mac.rs` adds the parts GPUI does not
set: `setHidesOnDeactivate(false)` so the panel survives another app taking
focus, hidden traffic lights, no AppKit shadow, and stationary collection
behaviour.

The transparency is per-pixel alpha from `WindowBackgroundAppearance::Transparent`.
The panel's fill colour carries the opacity; the text colour does not, which is
why the script stays legible at 10%.
