# Third-party notices

Motion Choreographer is released under the MIT licence (see `LICENSE`). A built app
also contains the following, each under its own licence.

## Fonts built into the app

| Font | Licence | Text |
| --- | --- | --- |
| Inter (variable, used at 400 and 700) | SIL Open Font License 1.1 | `crates/app/assets/fonts/Inter-LICENSE.txt` |
| JetBrains Mono (Medium) | SIL Open Font License 1.1 | `crates/app/assets/fonts/JetBrainsMono-OFL.txt` |
| Phosphor Icons, through the `egui-phosphor` crate | MIT | <https://github.com/phosphor-icons/core/blob/main/LICENSE> |
| Ubuntu Light, Hack, Noto Emoji and emoji-icon-font, through egui's default fonts | Ubuntu Font Licence 1.0, MIT, SIL Open Font License 1.1, MIT | <https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts/fonts> |

## The brand kit and the manuals site

| Font | Licence | Text |
| --- | --- | --- |
| Inter (variable), in `brand/fonts/Inter-Variable.ttf` and the manuals site (`manual/fonts/Inter-Variable.ttf`) | SIL Open Font License 1.1 | `brand/fonts/OFL.txt` (also `manual/fonts/Inter-OFL.txt`) |
| JetBrains Mono (Medium), for code in the manuals site | SIL Open Font License 1.1 | `manual/fonts/JetBrainsMono-OFL.txt` |

## Rust crates

The programs link the crates named in `Cargo.lock`. Most are under MIT,
Apache-2.0 or both. The `symphonia` crates (audio decoding) and `option-ext`
are under the Mozilla Public License 2.0: their source is unmodified and is
available from <https://crates.io>. None is under a GPL-only licence.

`cargo tree --format "{p} {l}"` lists every crate with its licence.

## Render comparison prototype

The standalone study in `design/render-prototype` includes Three.js 0.170.0
under the MIT licence; its licence is in
`design/render-prototype/vendor/THREE-LICENSE.txt`. It is not linked into the
desktop app.
