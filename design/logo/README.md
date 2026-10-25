# Addiem logo

Chosen 2026-10-25: mark **I02** with type **Newsreader**. The type was picked in round 11 and replaces B20 / Instrument Serif. Both come from the logo sheet.

- **Mark (I02).** A 4×4 dot grid. The dots on the diagonal grow toward the orange dot at the top right. The diagonal is the path to the day, the orange dot is the day itself (ad diem, "until the day"), and the grid is the calendar.
- **Type.** Newsreader 400, lowercase `addiem`. The dot of the i is an orange circle on the font's own dot, 135% of its size. The user wanted the lowercase a to look best; Newsreader won that comparison.

| File | Use |
|---|---|
| `addiem-mark.svg` / `-dark.svg` | 4×4 mark, 32px and up, light / dark background |
| `addiem-mark-small.svg` / `-small-dark.svg` | 3×3 mark (I01), 16–24px. At that size the 4×4 grid's light dots disappear |
| `addiem-icon.svg` | App icon source: the 4×4 mark on the accent tile. Used at 48px and up |
| `addiem-icon-small.svg` | Small app icon: the 3×3 mark on the same tile. Used at 24 and 32px |
| `addiem-icon-16.svg` | 16px app icon: three diagonal dots only, pixel-aligned (title bar). At 16px the 3×3 grid read as an empty tile (QA QB-06) |
| `icons/` | Copy of `src-tauri/icons`. Generated with `npx @tauri-apps/cli@2 icon design/logo/addiem-icon.svg -o design/logo/icons`. Then `32x32.png` was replaced with the small icon, and `icon.ico` was packed from three SVGs: 16 from the 16px icon, 24/32 from the small icon, 48/64/256 from the regular icon |

## Figma

The file "Addiem MVP UI" (xZsmRhJ4rvkjwYHfQyM5jx) has a page "Addiem 로고" (300:22727) with two frames:

- **"Addiem · 컴포넌트" (300:22886)** holds the components: Mark, Mark Dark, Mark Small, Mark Small Dark, App Icon, Wordmark, Wordmark Dark, Lockup and Lockup Dark.
- **The components are bound to color variables.** Mark, Mark Small and Wordmark use the `brand/path`, `brand/grid`, `brand/the-day` and `brand/ink` variables in the "Addiem Color" collection. One component therefore works in Light and Dark mode; App Icon keeps fixed colors.
- **The MVP Screens page uses them.** Its `Logo` component (32px) is an Addiem Mark (4×4) instance, and `TopNav` shows the Wordmark at 30px tall. The size was enlarged from 28 / 24 at the user's request; the bar stays 60px.
- **"Addiem 로고 시트" (301:22763)** is the sheet: lockups, marks, wordmarks, app icon sizes and colors. Its frames use the file's Addiem Color variables and Pretendard text styles. The dark tiles set the Dark mode explicitly.

## Colors

| Role | Light | Dark | On the icon tile |
|---|---|---|---|
| Accent (diagonal dots, icon tile) | #4433FF | #7A6DFF | white |
| Grid dots | #D9D5FF | #3A3470 | #6A5CFF |
| The day (orange) | #E05C06 (was #FF8A3D, darkened for 3:1 on the light top bar, QA QB-05) | #FF9A55 | #FF9A55 |
| Wordmark ink | #16151F | #ECEBF5 | — |

## Wordmark

| File | Use |
|---|---|
| `addiem-wordmark.svg` / `-dark.svg` | Type only |
| `addiem-lockup.svg` / `-dark.svg` | I02 mark + type |

- **Outlined.** The letters are paths, so the files render the same everywhere without the font. The paths come from Newsreader Regular (opsz 56, SIL OFL), drawn with fontTools.
- The orange dot is centered on the i's real tittle. Its radius, 6.08, is the size the user approved on the sheet (135% as measured there).
- Geometry at 56px, measured in the browser:
  - The advance of `add` is 91.34.
  - The i tittle sits at x 3–12, y −41 to −33 from the i's origin.
  - The orange dot is at (106.84, 20), r 6.08. The baseline is 57.
