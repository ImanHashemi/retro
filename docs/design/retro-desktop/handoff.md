# retro UI handoff — "desktop" direction

Reference mockups: `templates/desktop-home/DesktopHome.dc.html` (frames 1a–4c). Theme tokens + chrome recipe: `retro-theme.css` (copy into the single-file dashboard; it's vanilla CSS, no build step).

## Concept
The dashboard is a tiny retro desktop: a checkerboard "desk" holds beveled windows with center-title bars (old-Mac/CDE). Quiet instrument by default — one bright green accent carries all meaning (active, retro-owned, ok); red is reserved for genuine alerts.

## Structure
- **Top menu bar = the only navigation.** Overview · Knowledge · Activity · Config. Active tab gets a 2px accent underline. Right side: version · date · clock · `● hooks ok`.
- **Bottom "front panel" = pure status, never nav.** `▞ front panel | store N | queue N | budget N/10 | push Xh ago | ✓ healthy`. Cells turn `--alert` red when unhappy; the health slot is the loudest thing on screen (`⚠ budget exhausted · 7 queued`).
- **Windows never scroll internally on Overview.** They truncate with an honest count (`+ 3 more ▸ 21,3K`, `+ 11 more this week`) and a one-click path to the full view. Expansion happens in place (frame 4c); full lists live in Knowledge/Activity.
- Overview grid: Learned-this-week is the wide left window spanning both rows (1.45fr); Context, Retro-owns, Pipeline stack right.

## Chrome recipe (see retro-theme.css)
- Window: `--panel` bg, 2px bevel (`--bevel-hi/lo`), 1px `--line` outline.
- Title bar: 24px, horizontal pinstripes behind, title centered on a panel-colored patch, 11×11 close box left, meta right. Dark theme tints titles with the accent.
- Buttons: 1px border, `2px 2px 0` hard shadow, no radius; `:active` collapses the shadow. Danger = alert bg, bold.
- Bars: hard-edged segments — solid (`global`), 45° hatch (`project`), solid accent (`retro-owned`), horizontal accent stripes (`MEMORY.md`). Legend always shown; note the shared scale.

## Type
Mockups use Space Mono. Production (no webfonts): `ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`. Sizes: 26px stat numerals, 14px bar totals, 12px body, 11–11.5px secondary, 10px labels/legends (letter-spaced caps for column heads and day headers).

## States
- Selected row: `--accent-wash` bg + inset accent outline.
- Held rule: opacity .55 + "held" note; vetoed: opacity .4 + strikethrough.
- Veto confirm (frame 4c): modal window, dimmed desk, copy states the consequence ("blacklisted — never learned again"), danger button on the right.
- Confidence: 5-block glyph `■■■■□ .80` — text, not a widget.

## Data honesty rules
- No daemon: it's a hook-driven pipeline. Label stages `observe → analyze → project → push`.
- Counts get louder, layout never moves: fixed window heights, aggregate rows, bold footer counts.
