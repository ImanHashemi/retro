# Handoff: retro dashboard redesign — "desktop" direction

## Overview
Full redesign of the `retro ui` dashboard (the localhost UI served by the Rust binary in [ImanHashemi/retro](https://github.com/ImanHashemi/retro)). Four tabs — Overview, Knowledge, Activity, Config — restyled as a tiny retro desktop: a checkerboard "desk" holding beveled windows with center-title bars (old-Mac/CDE flavor), in light and dark themes, with a CSS-only skin system.

## About the Design Files
The files in this bundle are **design references created in HTML** — mockups showing intended look and behavior, not production code to copy directly. The task is to **recreate these designs inside retro's existing UI environment**: one self-contained HTML file served by the Rust binary, vanilla JS, no webfonts/CDN/images. `retro-theme.css` is written to that constraint and can be embedded as-is; the mockup markup uses inline styles and should be re-expressed using the classes in that stylesheet.

## Fidelity
**High-fidelity.** Colors, spacing, type sizes, chrome details, and copy are final. Recreate pixel-perfectly, except: mockups use the Space Mono webfont — production uses the system mono stack in `retro-theme.css` (`ui-monospace, SFMono-Regular, Menlo, Consolas, monospace`).

## Files in this bundle
- `DesktopHome.dc.html` — all 10 mockup frames on one canvas. Frame ids:
  - `1a`/`1b` Overview light/dark · `1c` Overview overflow/unhappy state
  - `2a` Knowledge · `2b` Activity (light)
  - `3a` Config (light) · `3b` Knowledge dark
  - `4a` Config dark · `4b` Activity dark · `4c` interactions (expanded list + veto modal)
  - Ignore the `<x-dc>`/`support.js`/`ds-base.js` scaffolding — it's the design tool's preview wrapper, not part of the design.
- `retro-theme.css` — design tokens (light + `[data-theme=dark]`) and chrome recipe classes. **Source of truth for all values.**
- `handoff.md` — concept, structure rules, chrome recipe, type scale, states, data-honesty rules.
- `skins.md` — skin system spec (CSS-only plugins in `~/.retro/skins/<name>/`) with vhs-84 example.

## Screens
### Overview (frames 1a/1b/1c)
Home answers "what's in my context right now". Grid: `1.45fr 1fr`, 12px gaps, 12px page padding.
- **Learned this week** (left, spans both rows): newest rules, each with name, scope·type tag, description, 5-block confidence glyph (`■■■■□ .80`), Veto button. Hard cap ~6 entries; footer shows `+ N more this week · N held` + link to Knowledge.
- **Context — session load** (right top): per-project stacked token bars — segments: solid `--bar-global` (global CLAUDE.md), 45° hatch (project CLAUDE.md), solid accent (retro-owned), horizontal accent stripes (MEMORY.md); legend below; shared 0–20K scale; top 5 by weight then dashed aggregate row `+ 3 more ▸ 21,3K`.
- **Retro owns** (right bottom-left): big stat numerals (26px) for tokens/rules/files + per-file list, top 3 + aggregate.
- **Pipeline** (right bottom-right): store nodes, session queue, AI budget today, `observe → analyze → project` last result, push. Red values when queue>0 / budget exhausted / stage failed (see 1c).

### Knowledge (2a/3b)
Left window (1.6fr): search + filter chips (scope/type/status/sort), rule table (RULE / SCOPE / CONFIDENCE / TOKENS / veto). Selected row: `--accent-wash` bg + inset accent outline. Held rows opacity .55; vetoed .4 + strikethrough. Footer: `showing N of 214 · 197 live · 12 held · 5 vetoed`. Right column: rule detail (text, status, type, confidence, evidence, first seen, last reinforced, cost; Veto/Edit/Pin buttons), History timeline, Store stats.

### Activity (2b/4b)
Left: day-grouped pipeline log (run ok/failed, push with token deltas +214/−126, veto events, decay sweeps), each row: time · description · status glyph (✓/✗/✋/♻). Right: This-week stats (runs/learned/decayed/vetoed, net context growth, AI spend), Health checks list, 4-week context-growth bar chart (hatched past weeks, solid accent current).

### Config (3a/4a)
Learning knobs (confidence-threshold slider at .70, AI-budget stepper, decay-window and token-cap dropdowns), Appearance (theme segment light/dark/auto, skin dropdown), Hooks & API (hook status, masked key, model dropdown), Projects table (owned files, token cap usage, learning toggle, Pause/Resume).

### Interactions (4c)
- Aggregate row expands in place; expanded state ends with underlined `show less ▴`.
- Veto confirm: modal window over desk dimmed with `rgba(38,35,28,.5)`; shows the rule, consequence copy ("Removed from ~/.claude/CLAUDE.md immediately and blacklisted — retro will never learn this rule again."), Cancel + danger button `Veto — never returns`.

## Structure rules (non-negotiable)
- **Top menu bar is the only nav** (active tab: 2px accent underline). Right side: `v0.4.2 · date · time · ● hooks ok`.
- **Bottom front panel is pure status, never nav**: `▞ front panel | store N | queue N | budget N/10 | push Xh ago | ✓ healthy`. Cells turn `--alert` when unhappy; health slot carries the loudest warning.
- **Windows never scroll internally on Overview** — truncate with honest counts, expand in place or link to the full tab. Fixed window heights; counts get louder, layout never moves.
- No "daemon" language anywhere: it's a hook-driven pipeline (`observe → analyze → project → push`, SessionEnd/SessionStart hooks).

## Chrome recipe (values in retro-theme.css)
- Window: `--panel` bg, 2px bevel (`--bevel-hi/--bevel-lo`), 1px `--line` outline. No border radius anywhere.
- Title bar: 24px; horizontal pinstripes; centered bold 11px title on a panel-colored patch; 11×11px close box left; 10px meta right. Dark theme: titles in `--accent`.
- Buttons: 1px `--line` border, `--field` bg, `2px 2px 0 var(--shadow)` hard shadow; `:active` collapses shadow + translates 2px. Danger: `--alert` bg, panel-colored bold text.
- Desktop: `repeating-conic-gradient(var(--desk-a) 0 25%, var(--desk-b) 0 50%) 0 0/8px 8px`.

## Design tokens
See `retro-theme.css` for the full set. Key values — Light: panel `#f4f2ea`, line `#4a463c`, ink `#26231c`, accent `#0da53c`, alert `#a8402e`, desk `#dcd9cf/#d7d4c9`. Dark: panel `#1e211a`, line `#3c4034`, ink `#d6d8cc`, accent `#2ee85c`, alert `#e85c48`, desk `#15170f/#181a12`. One accent only; red strictly for alerts.

## Type scale
13px/1.5 body base · 26px/700 stat numerals · 14px/700 bar totals · 12px table/body · 11–11.5px secondary · 10px letter-spaced caps for column heads, day headers, legends. All monospace.

## State management
- Theme: `data-theme="dark"` on root; `auto` follows `prefers-color-scheme`.
- Skin: config-persisted name; inline the skin CSS after base styles (see skins.md).
- Veto: optimistic removal + permanent blacklist; always via confirm modal.
- All data present in the existing UI's endpoints; this redesign adds no new data requirements beyond per-segment token counts for the context bars.

## Assets
None. No images or icon fonts — glyphs are unicode text (▞ ● ■ □ ✓ ✗ ✋ ♻ ⌕ ▸ ▴ ▾).
