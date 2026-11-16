# Motion Choreographer brand kit

Version 1.0, November 2026. PTZ and camera animation for live production.

The identity follows the Swiss (International Typographic) style: a mathematical grid, one grotesque typeface, flat geometric form, and a single signal colour that does a job instead of decorating.

## The idea

A keyframe held in an M.

The diamond is the keyframe symbol used in animation timelines. The M holds it in its notch. It is red because red means live in a control room. Everything is drawn from squares and 45° lines on a 12 × 12 unit grid.

## Logo

| Version | Use it for | Folder |
| --- | --- | --- |
| Primary lockup (horizontal, two lines) | Default. Websites, documents, splash screens | `logo/lockup-horizontal` |
| Stacked lockup | Narrow or tall spaces | `logo/lockup-stacked` |
| Inline lockup (one line) | Title bars, nav bars, footers | `logo/lockup-inline` |
| Mark | App icons, avatars, anywhere the name is already present | `logo/mark` |
| Mark, small | 24 px and below. Heavier stems so it holds up | `logo/mark-small` |
| Wordmark | When the mark already appears nearby | `logo/wordmark-*` |

Every version comes in four colourways, as SVG and transparent PNG:

| Colourway | Background |
| --- | --- |
| `color` | Paper and light greys |
| `color-reversed` | Ink and dark greys |
| `black` | One-colour printing on light |
| `white` | One-colour on dark, on Signal red, or over photos |

### Construction

| Part | Rule |
| --- | --- |
| Module | 12 × 12 units, square |
| Stem | 2u wide (3u in the small mark) |
| Diagonals | 45°, 3u deep |
| Keyframe | 4u square, rotated 45° |
| Gap | 1u between the M and the keyframe |

### Clear space and size

- Clear space: one keyframe (4u, a third of the mark's height) on every side.
- Minimum mark size: 16 px. Use the small mark at 24 px and below.
- Minimum lockup height: 24 px.
- The wordmark is always flush left. In the primary lockup the cap height of "Motion" meets the top of the mark and the baseline of "Choreographer" meets its foot.

### Misuse

Do not stretch, rotate, recolour the keyframe, add gradients, outline, or add shadows and effects. Do not centre the wordmark under the mark. Do not redraw the wordmark in another typeface.

## Colour

Ink and paper carry the layout. Red is the only accent.

| Name | HEX | RGB | Role |
| --- | --- | --- | --- |
| Ink | `#0A0A0A` | 10 10 10 | Ground and type |
| Paper | `#FFFFFF` | 255 255 255 | Ground and type |
| Signal | `#FF3000` | 255 48 0 | Live. The keyframe. The only accent |
| Grey 100 | `#F2F2F2` | 242 242 242 | Surface on light |
| Grey 300 | `#D4D4D4` | 212 212 212 | Rules on light |
| Grey 400 | `#A3A3A3` | 163 163 163 | Secondary text on dark |
| Grey 600 | `#6B6B6B` | 107 107 107 | Secondary text on light |
| Grey 800 | `#2E2E2E` | 46 46 46 | Rules on dark |
| Grey 900 | `#161616` | 22 22 22 | Surface on dark |

Functional signals, for the interface only:

| State | HEX | Meaning |
| --- | --- | --- |
| Program | `#FF3000` | On air, live |
| Preview | `#00C24E` | Next, ready |
| Caution | `#FFB800` | Warning |

Contrast (WCAG 2 ratios, computed from the values above):

| Pair | Ratio | Use |
| --- | --- | --- |
| Ink on Paper | 19.8:1 | Any text |
| Signal on Ink | 5.4:1 | Any text |
| Signal on Paper | 3.7:1 | Marks, rules, and type of 24 px or larger only |
| Paper on Signal | 3.7:1 | The white mark and type of 24 px or larger only |
| Ink on Signal | 5.4:1 | Any text. Use Ink for labels on red buttons and tags |
| Grey 400 on Ink | 7.8:1 | Secondary text on dark |
| Grey 600 on Paper | 5.3:1 | Secondary text on light |
| Preview on Ink | 8.3:1 | State labels |
| Caution on Ink | 11.4:1 | State labels |

Print values (Pantone, CMYK) are not specified here. Have your printer match Signal to the HEX value.

## Typography

Inter, in two weights: Regular 400 and Bold 700. Nothing else. The variable font and its SIL Open Font License are in `fonts/`.

| Style | Size / line | Weight | Tracking | Notes |
| --- | --- | --- | --- | --- |
| Display | 96 / 96 | 700 | −0.03 em | |
| Heading 1 | 64 / 64 | 700 | −0.03 em | |
| Heading 2 | 32 / 40 | 700 | −0.02 em | |
| Data | 24 / 32 | 400 | 0 | Tabular figures for time, angle, zoom |
| Body | 16 / 24 | 400 | 0 | At most 60 characters per line |
| Label | 12 / 16 | 400 or 700 | +0.08 em | Uppercase |

- Flush left, ragged right. Never centred, never justified.
- Hierarchy comes from size, weight, and position. Not from colour, not from a third weight.
- Turn on tabular figures wherever numbers change live: `font-variant-numeric: tabular-nums`.

## Interface

1. Grid. 8 px unit, 12 columns, 24 px gutters. Everything snaps.
2. Corners. Radius 0. No shadows, no gradients, no glow.
3. Rules. 1 px lines divide the surface. No card frames.
4. Colour. Red means live. Green means preview. Nothing else is coloured.
5. Type. Inter 400 and 700. Uppercase labels. Tabular figures for time and angle.
6. Motion. 120 to 200 ms, linear or ease-out, no bounce.

Dark (Ink ground) is the default theme because control rooms are dark. The light theme is for documents and print.

Tokens are in `tokens/` as CSS custom properties, W3C-style JSON, and a Tailwind preset.

```html
<link rel="stylesheet" href="tokens/tokens.css">
<body data-theme="light"> <!-- omit for the default dark theme -->
```

## Layout for banners and covers

Anchor the mark top left and the name bottom left. Leave the right side open. One rule, one descriptor line, nothing centred. Red appears once, in the keyframe.

## Using it in the repo

README header that follows the reader's GitHub theme:

```html
<picture>
  <source media="(prefers-color-scheme: dark)" srcset="brand/social/readme-banner-dark.png">
  <img src="brand/social/readme-banner-light.png" alt="Motion Choreographer" width="640">
</picture>
```

- Repository social preview: upload `social/github-social-preview.png` (1280 × 640) under Settings, Social preview.
- Web: copy `icon/web/` to the site root and add

```html
<link rel="icon" href="/favicon.ico" sizes="48x48">
<link rel="icon" href="/favicon.svg" type="image/svg+xml">
<link rel="apple-touch-icon" href="/apple-touch-icon.png">
<link rel="manifest" href="/site.webmanifest">
```

- Desktop builds: `icon/macos/AppIcon.icns` and `icon/windows/app.ico`. The macOS icon uses a rounded tile because the platform expects one; everywhere else the icon is a full-bleed square.

## Files

```
logo/          mark, mark-small, three lockups, two wordmarks (SVG + PNG, four colourways each)
icon/          app icon SVG, PNG 16 to 1024, web favicons and manifest, macOS .icns, Windows .ico
social/        GitHub social preview, README banners (dark and light)
tokens/        tokens.css, tokens.json, tailwind.preset.js
fonts/         Inter variable font, OFL licence
guidelines/    six guideline boards (PNG, SVG sources)
BRAND.md       this file
```

## References

Swiss style guides consulted for these rules:

- Superdesign, "Swiss Style" design system: grid as law, one signal red used as a functional signal, radius 0, no shadows, uppercase labels, numbered sections. https://superdesign.dev/library/swiss-style
- Curio, "What is Swiss International Style?": asymmetric balance, white space as structure, colour that earns its place by doing a specific job. https://designbycurio.com/learn/swiss-international
- StyleKit, "Swiss International UI Style": weights 400 and 700 only, red as the only accent, rules instead of card frames. https://www.stylekit.top/en/styles/swiss-style
- DESIGN.md library, "Minimalism & Swiss Style": spacing in multiples of a 4 or 8 px base unit, one typeface, fast transitions with no bounce. https://designmd.app/library/minimalism-swiss-style
- Design Style Book, "Modernist Logo Design": reductive geometric construction, flat single-colour execution, negative space, scalable form. https://www.designstylebook.com/graphic-design/logo-modernism
- Wikipedia, "International Typographic Style": asymmetric layouts, grid, sans-serif type, flush left and ragged right. https://en.wikipedia.org/wiki/International_Typographic_Style
- Inter typeface by The Inter Project Authors, SIL Open Font License 1.1. https://github.com/rsms/inter
