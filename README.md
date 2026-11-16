# Prompter

Floating, translucent script overlay for macOS that stays over full-screen apps without taking focus. Built in Rust with [GPUI](https://github.com/zed-industries/zed).

![Prompter](assets/screenshot.png)

- Non-activating `NSPanel` across all spaces, with no Dock icon or menu bar
- Backdrop fades from 15% to 100% while text stays at full contrast
- Any installed font (`10` to `120 pt` in `1 pt` steps), 5 weights, and `*italic*` / `**bold**` Markdown
- Saves geometry, font, opacity, and last file in `~/Library/Application Support/Prompter/settings.json`

## Controls

| Action | Input |
| --- | --- |
| Move | Drag grip or empty top strip |
| Resize | Drag any edge or corner |
| Minimize | Minimize button or `Cmd M` |
| Font size | `−` / `+`, `Cmd +` / `Cmd -`, `Cmd 0` |
| Opacity | Strip slider |
| Font & weight | `Aa` (`↑` / `↓` / `Return`) and weight chips |
| Load script | `Cmd V`, `Cmd O`, drop a file, or CLI path |
| Scroll | Wheel, `↑` / `↓`, `PageUp` / `PageDown`, `Home` |
| Quit | `×` or `Cmd Q` |

## Build

Requires Rust and Xcode Command Line Tools on macOS:

```sh
cargo build --release
./scripts/bundle.sh target/release/prompter
open dist/Prompter.app
```

Cross-building for Apple Silicon on Linux (`clang`, `lld`, `aarch64-apple-darwin`, and `MacOSX14.5.sdk`):

```sh
rustup target add aarch64-apple-darwin
MAC_SDK=/path/to/MacOSX14.5.sdk ./scripts/build-mac.sh
```

If `gpui_apple` fails on a missing `stitched_shaders.metal`, run `MAC_SDK=/path/to/MacOSX14.5.sdk ./scripts/stitch-shaders.sh` once.
