# retro skins — spec

Skins re-dress the dashboard without touching markup or JS. A skin is a folder in `~/.retro/skins/<name>/`:

```
~/.retro/skins/vhs-84/
├── skin.toml      # name, author, version, base = "light" | "dark"
└── skin.css       # the whole skin
```

## Contract
- The base UI styles everything through the custom properties and classes in `retro-theme.css` (`--panel`, `--accent`, `.window`, `.titlebar`, …). A skin is **CSS only** — it overrides those properties and may restyle the recipe classes; it cannot add scripts (the dashboard stays one self-contained HTML file; `retro ui` inlines the chosen skin's CSS after the base styles).
- Minimum viable skin = redefine the `:root` token block. Everything else is optional garnish.
- Layout is owned by the app: skins must not `display:none` data or reorder nav. Decorative pseudo-elements/overlays are fair game.
- `skin.toml` `base` picks which built-in token set the skin inherits, so a 10-line skin still gets sane fallbacks.

## Selection
`retro ui --skin vhs-84`, or persisted via Config → Appearance → skin (the dropdown in frame 3a/4a). Invalid/missing skin falls back to `desktop` with a front-panel warning, never a crash.

## Example: vhs-84 (the 6a mockup as a skin)
```css
/* skin.css — vhs-84 */
:root{
  --desk-a:#0a0118; --desk-b:#0d0220;
  --panel:#140629; --line:#ff2fd6; --line-soft:#7a2f8f;
  --bevel-hi:#ff2fd6; --bevel-lo:#3d0d5e;
  --ink:#e8d6ff; --ink-soft:#b48fd9; --ink-faint:#7a5f99;
  --accent:#22e6e6; --accent-wash:#12203a; --alert:#ff3355;
  --chip:#1d0a38; --field:#0a0118; --bar-global:#5a2d8a;
  --shadow:rgba(255,47,214,.35);
}
/* garnish: scanlines + glow */
body::after{content:"";position:fixed;inset:0;pointer-events:none;
  background:repeating-linear-gradient(0deg,transparent 0 2px,rgba(0,0,0,.18) 2px 3px)}
.titlebar .title{text-shadow:0 0 6px var(--accent)}
.window{box-shadow:0 0 12px rgba(255,47,214,.25)}
```

## Why this shape
Keeps the core UI a quiet instrument while letting the community go loud; a skin can't break data display, only re-paint it — so vetoing a rule looks the same in `desktop` and in neon.
