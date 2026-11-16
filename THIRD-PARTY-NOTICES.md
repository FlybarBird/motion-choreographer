# Third-party notices

Motion Choreographer is released under the MIT licence (see `LICENSE`). A built app
also contains the following, each under its own licence.

## Fonts built into the app

| Font | Licence | Text |
| --- | --- | --- |
| Inter (Regular, Bold, and Regular with tabular figures; fixed weights made from the variable font in `brand/fonts`) | SIL Open Font License 1.1 | `crates/app/assets/fonts/Inter-LICENSE.txt` |
| Phosphor Icons, through the `egui-phosphor` crate | MIT | <https://github.com/phosphor-icons/core/blob/main/LICENSE> |
| Ubuntu Light, Hack, Noto Emoji and emoji-icon-font, through egui's default fonts | Ubuntu Font Licence 1.0, MIT, SIL Open Font License 1.1, MIT | <https://github.com/emilk/egui/tree/main/crates/epaint_default_fonts/fonts> |

## Rust crates

The programs link the crates named in `Cargo.lock`. Most are under MIT,
Apache-2.0 or both. The `symphonia` crates (audio decoding) and `option-ext`
are under the Mozilla Public License 2.0: their source is unmodified and is
available from <https://crates.io>. None is under a GPL-only licence.

`cargo tree --format "{p} {l}"` lists every crate with its licence.
