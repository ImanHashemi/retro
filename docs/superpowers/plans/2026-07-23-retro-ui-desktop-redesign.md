# Retro dashboard "desktop" redesign Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax.

**Goal:** Reskin the `retro ui` dashboard to the approved "desktop" direction (retro-desktop chrome, four tabs Overview/Knowledge/Activity/Config, light+dark), bound entirely to real backend data, plus one small config-write endpoint so the Config knobs persist. Ship as UI-focused 3.1.0.

**Architecture:** The dashboard stays ONE self-contained HTML file (`crates/retro-cli/src/ui/assets/index.html`) — vanilla JS, inline CSS, no webfonts/CDN/images. The design's `retro-theme.css` is embedded verbatim (system mono stack, no Space Mono). Every surface reads only fields the API actually returns; features retro has no backend for (decay, confidence-history, weekly growth chart, AI-dollar spend, skins, token caps, project pause/resume, API-key management) are **omitted or shown as an honest empty/planned state — never faked**. Three small, honest backend additions: `GET`/`POST /api/config` (whitelisted fields the pipeline actually reads), `total_nodes` + store breakdown on `/api/xray`, `budget_max` on `/api/health`.

**Tech Stack:** Rust (retro-cli, tiny_http sync), vanilla JS + CSS in one embedded HTML file.

**Design source of truth (read before any UI task):** `docs/design/retro-desktop/` in this branch — `README.md`, `handoff.md`, `retro-theme.css`, `skins.md`, and `DesktopHome.dc.html` (frames 1a–4c). Fidelity is HIGH: colors/spacing/type/copy are final; the only intended deviation is the font (system mono, not Space Mono).

---

## Context for implementers (read first)

- **Conventions:** `CoreError` + `?` in retro-core, `anyhow` in retro-cli. NEVER `cargo fmt`; `rustfmt --edition 2024` on NEW Rust files only, manual style on edits. Mandatory preflight every dispatch: `cd <repo>/.claude/worktrees/v3-ui-redesign && git branch --show-current` must print `v3-ui-redesign`.
- **SAFETY (absolute):** never run `retro init/start/stop/migrate/uninstall` against the real machine. Live UI checks (`retro ui`) ONLY with an isolated `RETRO_HOME` temp dir + a config whose `[paths] claude_dir` points at a temp dir; kill only the `retro ui` PID you spawn; use a non-default `[ui] port`. Never bare `git stash`. Never stage `Cargo.lock` unless a task says so (none here — no dependency changes).
- **The existing file you are replacing:** `crates/retro-cli/src/ui/assets/index.html` (current 3.0.0 dashboard — X-ray/Knowledge/Health/History). Preserve its hard-won JS discipline when you rewrite: `esc()` escaping of ALL server strings (it escapes `& < > " '`), the `get()/post()` fetch helpers, the sequence-guarded `render()` (stale response can't overwrite a newer one), focus/cursor restore after re-render, `getProjectSlugs` no-cache-on-failure. `.hidden { display:none !important }` (the modal-backdrop fix — keep the `!important`).
- **The exact current API (verified in `crates/retro-cli/src/ui/api.rs`):**
  - `GET /api/xray` → `{global_claude_md:{present,bytes,tokens_est}, global_active_nodes, skills_count, projects:[{slug,path,claude_md,claude_local_md,memory_md,active_nodes}], store_warnings}`. File objects are `{present:true,bytes,tokens_est}` or `{present:false}`. `tokens_est = bytes/4`.
  - `GET /api/nodes?scope=&type=&active=&q=` → `[{id,scope,type,confidence,active,updated,body,sources}]` (body truncated 200; `type` ∈ rule|preference|pattern|memory; scope = `global` | `project/<slug>`). 409 if index not built.
  - `GET /api/node?scope=&id=` → `{id,scope,type,confidence,sources,created,updated,invalidated_by,body,path}`.
  - `GET /api/health` → `{stages:{<name>:{at,ok,detail}}, queue_len, budget_remaining, notifications_pending}`.
  - `GET /api/history?limit=` → `[{hash,date,subject}]` (git log of the store; subjects like `retro: learn 2 node(s), update 1`, `user: invalidate <id> (dashboard)`, `retro: maintenance`, `retro: migrate v2 knowledge`, `user: edit <id> (dashboard)`).
  - `GET /api/doctor` → `{checks:[{name,ok,detail}]}`.
  - `POST /api/node/invalidate {scope,id}` · `POST /api/node/update {scope,id,body?,confidence?}` · `POST /api/project/exclude {slug}`. All take `run.lock`; return `{ok:true}` or `{error}` (400/404/409/500/503).
- **Config fields that exist and are read by the pipeline** (`crates/retro-core/src/config.rs`): `knowledge.confidence_threshold` (f64, default 0.7 — the projection/held gate), `runner.max_ai_calls_per_day` (u32, default 10), `ai.model` (String, default "sonnet"), `ai.backend`, `analysis.window_days`/`analysis.staleness_days`, `paths.claude_dir`, `privacy.exclude_projects`, `ui.port`. `Config::load(&Path)` / `Config::save(&self,&Path)` exist; `retro_dir()` honors `$RETRO_HOME`. **There is NO decay, token-cap, skin, API-key, pause/resume, dollar-cost, weekly-snapshot, or confidence-history feature anywhere in retro-core (grep-confirmed).**
- **Honest-treatment table (BINDING — every UI task obeys this):**

  | Design element | Treatment |
  |---|---|
  | Overview: Context bars (global/project/retro/MEMORY segments) | REAL — `/api/xray` per-file `tokens_est`: global=global_claude_md, project=claude_md, retro-owned=claude_local_md, MEMORY=memory_md. Per-project total = sum. |
  | Overview: Retro owns (tok/rules/files + per-file) | REAL — sum tokens_est of retro-owned files (global CLAUDE.md + each CLAUDE.local.md); rules = total active nodes; files = count present owned files. **MEMORY.md is NOT retro-owned (Claude Code owns it) — exclude it from "retro owns"; it may still appear in the Context bar as a segment.** |
  | Overview: Learned this week | REAL — `/api/nodes` filtered client-side by `updated` within 7 days; "held" = active && confidence < threshold; Veto = invalidate. |
  | Overview/front-panel: store N / queue / budget N/max / push / health | REAL — store=`xray.total_nodes` (new), queue=`health.queue_len`, budget=`health.budget_remaining`/`budget_max` (new), push=relative time from `health.stages.push.at`, health=worst of doctor/stages/queue/budget. |
  | Knowledge: table + filters + veto + detail(text/status/type/confidence/cost/created/updated/sources/path) | REAL — cost = token-est of body (chars/4). Status live/held/vetoed derived from active+confidence+threshold. Edit=update endpoint. |
  | Knowledge: store stats live/held/vetoed | REAL — from `xray.store` breakdown (new). **"decayed out (30d)" — OMIT (no decay).** |
  | Knowledge: rule "History" timeline (.89→.92, promoted) & "seen 7× across 3 projects" | **OMIT — not tracked. Detail shows created/updated/sources count instead.** |
  | Knowledge: "Pin ↑" button | **OMIT — no pin feature.** Keep Veto + Edit. |
  | Activity: pipeline log | REAL but coarser — from `/api/history` commit subjects, day-grouped, classified (learn→run ok ✓, user:invalidate→veto ✋, maintenance→♻, migrate/edit shown plainly). **No fabricated token deltas / candidate breakdowns.** |
  | Activity: This-week (runs/learned/vetoed) | REAL — counted/parsed from history within 7 days. **"decayed", "net context growth", "AI spend $" — OMIT (no decay / no token snapshots / no dollar tracking). "AI calls" may show budget used today (real).** |
  | Activity: Health checks | REAL — `/api/doctor` checks. Version = crate version (real). **"0.4.3 available" update-check — OMIT.** |
  | Activity: 4-week context-growth chart | **OMIT the chart (no historical snapshots). Replace panel body with an honest one-liner or drop the panel.** |
  | Config: confidence threshold / AI budget / model | REAL & WRITABLE via new `POST /api/config`. |
  | Config: theme light/dark/auto | REAL — client-side, localStorage + `data-theme`, auto=prefers-color-scheme. |
  | Config: decay window / token cap / API-key change / skin selector | **OMIT (no backend). Skins may get ONE honest "planned" line (skins.md is a real future spec) — never a dropdown that fakes a current value.** |
  | Config: Hooks status | REAL — `/api/doctor` hooks check. |
  | Config: Projects table | REAL — files/tokens from xray; action = Exclude (existing destructive confirm). **No token-cap denominator, no pause/resume toggle (no such feature).** |
  | Skins system end-to-end | **OUT OF SCOPE — its own future plan.** |

  A UI task that cannot source a value from the list above MUST omit the element (or render an explicit empty/planned state), never invent data. When in doubt, omit and note it in the task report.

---

### Task 1: Config read/write endpoint + xray/health honest extensions

**Files:** Modify `crates/retro-cli/src/ui/api.rs` (+ handlers, + route wiring, + tests).

- [ ] **Step 1: Write failing handler tests** in `api.rs`'s `#[cfg(test)] mod tests` (mirror the existing handler-test style — TempDir store, `Config` with temp `claude_dir`):

```rust
#[test]
fn config_get_returns_whitelisted_fields() {
    let tmp = TempDir::new().unwrap();
    let mut config = Config::default();
    config.knowledge.confidence_threshold = 0.7;
    config.runner.max_ai_calls_per_day = 10;
    config.ai.model = "sonnet".to_string();
    let (body, status) = api_config_get(&config);
    assert_eq!(status, 200);
    assert_eq!(body["confidence_threshold"], 0.7);
    assert_eq!(body["max_ai_calls_per_day"], 10);
    assert_eq!(body["model"], "sonnet");
    assert!(body["models"].as_array().unwrap().iter().any(|m| m == "sonnet"));
}

#[test]
fn config_post_persists_whitelisted_fields_only() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.toml");
    Config::default().save(&cfg_path).unwrap();
    // needs run.lock parent
    let (body, status) = api_config_post(
        tmp.path(),
        &json!({"confidence_threshold": 0.85, "max_ai_calls_per_day": 20, "model": "haiku"}),
    );
    assert_eq!(status, 200, "{body:?}");
    let reloaded = Config::load(&cfg_path).unwrap();
    assert!((reloaded.knowledge.confidence_threshold - 0.85).abs() < 1e-9);
    assert_eq!(reloaded.runner.max_ai_calls_per_day, 20);
    assert_eq!(reloaded.ai.model, "haiku");
}

#[test]
fn config_post_validates_ranges_and_model() {
    let tmp = TempDir::new().unwrap();
    Config::default().save(&tmp.path().join("config.toml")).unwrap();
    assert_eq!(api_config_post(tmp.path(), &json!({"confidence_threshold": 1.5})).1, 400);
    assert_eq!(api_config_post(tmp.path(), &json!({"confidence_threshold": -0.1})).1, 400);
    assert_eq!(api_config_post(tmp.path(), &json!({"max_ai_calls_per_day": 100000})).1, 400);
    assert_eq!(api_config_post(tmp.path(), &json!({"model": "gpt-4"})).1, 400);
    // empty patch is a no-op success
    assert_eq!(api_config_post(tmp.path(), &json!({})).1, 200);
}

#[test]
fn xray_reports_total_and_store_breakdown() {
    // build a store with 1 live (conf>=thr), 1 held (conf<thr), 1 invalidated node
    // (reuse the xray test scaffold); assert body["total_nodes"]==3 and
    // body["store"]=={"live":1,"held":1,"vetoed":1}
}
```

- [ ] **Step 2: Run, verify fail** (handlers undefined): `cargo test -p retro-cli config_ xray_reports` → compile error.

- [ ] **Step 3: Implement.**
  - `api_config_get(config: &Config) -> (Value, u16)`: return `{confidence_threshold, max_ai_calls_per_day, model, models:[...]}` where `models` is the allowed set: `["sonnet","haiku","opus"]` (match the strings `ai.model` accepts; keep it a small const `ALLOWED_MODELS`). 200.
  - `api_config_post(store_root: &Path, body: &Value) -> (Value, u16)`: take `run.lock` (same helper as other writes → 503 if busy); `Config::load(store_root.join("config.toml"))`; for each present key validate & apply — `confidence_threshold` f64 in `[0.0,1.0]` else 400; `max_ai_calls_per_day` integer in `[0,1000]` else 400; `model` must be in `ALLOWED_MODELS` else 400; unknown keys ignored. `config.save(...)`. Return the new `api_config_get(&config)` body, 200. Malformed/absent field types → 400.
  - Wire routes: `(Method::Get, "/api/config") => api_config_get(config)`; `(Method::Post, "/api/config") => read_json_body then api_config_post(store_root, &body)`. (Note `route()` already has `config: &Config` in scope.)
  - `api_xray`: it already does `store.load_all()`. Add `total_nodes = loaded.nodes.len()`, and a `store` object `{live, held, vetoed}` computed over `loaded.nodes` using `config.knowledge.confidence_threshold`: vetoed = `!n.is_active()`; among active, held = `confidence < threshold`, live = `>= threshold`. Add both to the returned JSON.
  - `api_health`: add `"budget_max": config.runner.max_ai_calls_per_day` alongside `budget_remaining`.

- [ ] **Step 4: Tests green** — `cargo test` (expect baseline + 4 new). `rustfmt --edition 2024` is NOT needed (editing an existing file — manual style).

- [ ] **Step 5: Commit** — `git add crates/retro-cli/src/ui/api.rs && git commit -m "feat(ui): config read/write endpoint; xray total/store breakdown; health budget_max"` (Co-Authored-By trailer).

---

### Task 2: Desktop chrome foundation — theme, menu, front panel, tab shell

**Files:** Rewrite `crates/retro-cli/src/ui/assets/index.html` (foundation only; tab bodies are stubs this task).

Read `docs/design/retro-desktop/retro-theme.css` and `handoff.md` first. Embed `retro-theme.css` **verbatim** in a `<style>` block (it already uses the system mono fallback after Space Mono — leave the `--font` line as-is; the webfont simply won't load and the fallback applies). Keep the CSS reset minimal (`*{box-sizing}` is already in the theme).

- [ ] **Step 1: Head + theme plumbing.** `<!doctype html>`, `<head>` charset/viewport, `<title>retro</title>`, embedded theme CSS. Theme state: read `localStorage.getItem('retro-theme')` (`light`|`dark`|`auto`, default `auto`); apply by setting `data-theme` on `<html>` — for `auto`, resolve via `matchMedia('(prefers-color-scheme: dark)')` and also listen for changes. Expose `setTheme(mode)` that persists + re-applies. (Config tab wires the segmented control to this in Task 6.)
- [ ] **Step 2: Menu bar** (`.menubar`, frame 1a header): `▞ retro` brand, four tabs `Overview Knowledge Activity Config` (active gets `.tab.active` accent underline), right meta `v<VERSION> · <date> · <clock> · ● hooks ok`. VERSION is injected — use a `<span id="ver">` filled from `/api/doctor` (the `claude-cli`/version check) OR hardcode from a `data-` attr the server could set; simplest honest: fetch `/api/health` isn't versioned, so read the crate version from a new tiny field OR display the running binary version via a `<!--VERSION-->` token the Rust side does NOT currently template. **Decision:** show `v3.1.0` sourced from a `const VERSION` in the JS that the release task keeps in sync, OR omit the version if that's the only non-live bit — keep it, hardcode `VERSION` const, note in report. date/clock from `new Date()` updated each minute. `● hooks ok` / `● hooks ?` from `/api/doctor` hooks check (green accent if ok, alert if not).
- [ ] **Step 3: Front panel** (`.front-panel`, bottom, frame 1a footer): `▞ front panel | store N | queue N | budget N/max | push Xh ago | <health>`. Populate from `/api/xray` (total_nodes), `/api/health` (queue_len, budget_remaining, budget_max, stages.push.at). Health slot = loudest state: if a doctor check failed OR queue>0 OR budget exhausted → `.alert` red with the loudest message (e.g. `⚠ budget exhausted · N queued`), else `✓ healthy`. Cells for queue/budget turn `.alerted` red when unhappy (queue>0, budget 0). **This is pure status, never nav** (handoff rule).
- [ ] **Step 4: Tab shell + router.** `<main id="main">` holds the active tab; `tab` state (`overview` default), `renderSeq` guard, `render()` dispatches to `views[tab]()` (stubs returning a placeholder `.window` this task), focus-restore preserved. Menu clicks switch tab + re-render + update active underline. Front panel + menu refresh on an interval (e.g. 30s) and after any write. Keep `esc/get/post` helpers.
- [ ] **Step 5: Verify (isolated live check).** `cargo build --release`. Seed an isolated `RETRO_HOME` (temp) + temp `claude_dir` + `config.toml` pointing at it + one global node + `retro reindex`; `RETRO_HOME=… retro ui --no-open &`; curl `/` (expect the new HTML), `/api/config` (200), `/api/health` (has budget_max), `/api/xray` (has total_nodes/store). Kill the PID. Confirm the shell renders (chrome/menu/front-panel present in the served HTML; grep for `.menubar`/`front-panel`/`data-theme`). Paste outputs.
- [ ] **Step 6: Commit** — `feat(ui): desktop chrome foundation — theme/menu/front-panel/tab shell`.

---

### Task 3: Overview tab (frames 1a/1b/1c) — the fully-real tab

**Files:** Modify `index.html` (`views.overview`).

Read frames `1a` (light), `1b` (dark), `1c` (busy/overflow) in `DesktopHome.dc.html`. Grid `1.45fr 1fr`, 12px gaps/padding. Re-express inline styles as the theme's recipe classes (`.window`, `.titlebar`+`.box`+`.title`+`.meta`, `.bar`+`.seg-*`, `.btn`).

- [ ] **Step 1:** Fetch `/api/xray` + `/api/nodes?active=true` + `/api/health` + `/api/config` (threshold). Build the four windows:
  - **Learned this week** (left, spans both rows): nodes with `updated` within 7 days, newest first, cap 6; each row = `<b>id</b> scope·type`, confidence glyph (5 blocks `■`/`□` from `Math.round(confidence*5)` + `.NN`), Veto `.btn`, description = body (truncated). Held rows (active && confidence<threshold) at opacity .55 with a "held" tag. Footer: `+ N more this week · N held · view all in Knowledge →` (link switches tab to knowledge). Title meta `N new · N held`.
  - **Context — session load** (right top): per-project stacked bar, segments widths from the four `tokens_est` values on a shared 0–20K scale (`width = min(tok/20000,1)*100%`); grid `150px 1fr 68px` (name / bar / total). Legend row (4 swatches + `scale 0 – 20K`). Top 5 projects by total weight; if more, dashed aggregate row `+ N more ▸ <sum>` (expand-in-place per frame 4c: clicking expands to all, ending with `show less ▴`). Meta `top 5 of N · by weight` or `tokens at session start`.
  - **Retro owns** (right bottom-left): 26px numerals — tokens (sum tokens_est of global CLAUDE.md + all CLAUDE.local.md; **exclude MEMORY.md**), rules (total active nodes), files (count present owned files); then per-file list `path` → `nodes · tokens`, top 3 + `+ N more files` aggregate.
  - **Pipeline** (right bottom-right): store `N nodes`, session queue `N`, AI budget today `used/max`, `observe → analyze → project` `✓/✗ HH:MM` (from health.stages — worst/last stage), push `Xh ago ✓`. Red (`--alert`) values when queue>0 / budget exhausted / a stage failed (frame 1c).
- [ ] **Step 2:** Windows never scroll internally — fixed heights, truncate with honest counts (handoff rule). Verify Learned-this-week caps at 6 and the Context aggregate row appears only when >5 projects.
- [ ] **Step 3: Verify (isolated live check).** Seed a store with ~3 projects (varied file sizes) + several recent nodes (some below threshold) via temp RETRO_HOME; `retro ui`; screenshot-equivalent: curl `/` and confirm structure, then (if feasible) eyeball in a browser against frame 1a. Paste the rendered Overview HTML for the seeded data and confirm numbers match the seed. Test the unhappy state by seeding queue>0 (enqueue a session) + confirm front panel + pipeline go red.
- [ ] **Step 4: Commit** — `feat(ui): Overview tab — context bars, retro-owns, learned-this-week, pipeline`.

**→ CHECKPOINT: pause here for the user to eyeball Overview before Knowledge/Activity/Config.**

---

### Task 4: Knowledge tab (frames 2a/3b)

**Files:** Modify `index.html` (`views.knowledge`, veto modal, node detail).

- [ ] **Step 1:** Left window (1.6fr): search input (`⌕`) + filter chips (scope/type/status/sort) driving `/api/nodes?scope=&type=&q=` (+ client sort). status chip: live/held/vetoed/all → maps to `active` param + client threshold split (live=active&≥thr, held=active&<thr, vetoed=!active). Table columns `RULE / SCOPE / CONFIDENCE / TOKENS / veto` (10px letter-spaced caps heads). Rows: id+description, scope, confidence glyph, token-est (body chars/4; `—` for held/vetoed per frame), Veto btn (`✗` glyph for already-vetoed). Selected row `.row-selected`; held `.row-held`; vetoed `.row-vetoed` (strikethrough). Footer `showing N of TOTAL · L live · H held · V vetoed` (from xray.store) + `next page →` (client paging).
- [ ] **Step 2:** Right column: **Rule detail** (`/api/node`): boxed body text; grid of status(`live · in <path>` / `held` / `vetoed`), type, confidence glyph, sources (`N source session(s)`), first seen (`created`), last updated (`updated`), cost (`N tokens`); buttons Veto + Edit text (Edit → prompt/inline → `/api/node/update {body}`). **No Pin, no History timeline, no "seen N× across M projects".** **Store** window: live/held/vetoed from xray.store. **No "decayed out" row.**
- [ ] **Step 3:** **Veto-confirm modal** (frame 4c): `.overlay` dimmed desk + `.dialog .window` centered; shows the rule box + consequence copy "Removed from `<file>` immediately and blacklisted — retro will never learn this rule again." Cancel `.btn` + `.btn-danger` "Veto — never returns". On confirm → `/api/node/invalidate` → close, re-render, refresh front panel. (Invalidate is retro's real permanent-removal; "blacklist/never learn again" copy is aspirational — soften to match real behavior: "Removed from `<file>` and marked invalid — it won't be re-projected." Keep it honest; do not claim a blacklist that doesn't exist.) `exclude` on the Config projects table reuses the same modal pattern.
- [ ] **Step 4: Verify** isolated: seed live+held+invalidated nodes; confirm filters, selection, detail fields, and the veto flow end-to-end (node inactive after, front panel store count updates). Paste outputs.
- [ ] **Step 5: Commit** — `feat(ui): Knowledge tab — rule store, detail, veto modal`.

---

### Task 5: Activity tab (frames 2b/4b) — honest pipeline log

**Files:** Modify `index.html` (`views.activity`).

- [ ] **Step 1:** Left window: **Pipeline log** from `/api/history?limit=50`, day-grouped by `date` (10px caps day headers `TODAY — WED JUL 17` / `TUE JUL 16`), each row `time · description · glyph`. Classify by subject: `retro: learn N node(s), update M` → `run ok` ✓ (show the parsed "N learned · M updated"); `user: invalidate <id>` → `veto` ✋ (strikethrough id); `retro: maintenance` → `♻`; `retro: migrate…`/`user: edit…`/`retro: import…` → plain row with the real subject. **Do NOT fabricate token deltas or candidate/promoted breakdowns** — show only what the subject carries. Footer `N runs · N ok · N veto` counted from the window.
- [ ] **Step 2:** Right column: **This week** — 26px numerals for runs (count of `retro:` commits within 7d), learned (sum of parsed "learn N" within 7d), vetoed (count `user: invalidate` within 7d); sub-line `AI calls today: used/max` (from health budget). **OMIT decayed / net-growth / dollar-spend.** **Health checks** — list from `/api/doctor` checks (`✓/✗` + name + detail), plus `version <VERSION>` (no update-check). **Context growth** — **OMIT the 4-week chart** (no snapshots); either drop this window or render one honest line: "context-growth history isn't tracked yet". Prefer dropping the window to avoid a dead panel; note the choice in the report.
- [ ] **Step 3: Verify** isolated: seed a store with several commits of varied subjects (learn/invalidate/maintenance) across a couple of days; confirm grouping, classification, counts. Paste outputs.
- [ ] **Step 4: Commit** — `feat(ui): Activity tab — pipeline log, this-week, health (honest, no decay/dollar/chart)`.

---

### Task 6: Config tab (frames 3a/4a) — working knobs, honest omissions

**Files:** Modify `index.html` (`views.config`).

- [ ] **Step 1:** **Learning** window: confidence-threshold slider (real `<input type=range min=0 max=1 step=.01>` styled to the frame's beveled track/handle; label `.NN`) → on change `POST /api/config {confidence_threshold}`; AI-budget stepper (`− N +`) → `POST /api/config {max_ai_calls_per_day}`. **OMIT decay-window and token-cap** (no backend). Initial values from `GET /api/config`.
- [ ] **Step 2:** **Appearance** window: theme segmented control light/dark/auto wired to `setTheme()` (Task 2), reflecting current mode. **Skin:** one honest line — "Skins (community themes in `~/.retro/skins/`) — planned" (skins.md is a real future spec); NO dropdown that fakes a selection.
- [ ] **Step 3:** **Hooks & API** window: hook status `✓ installed` / `✗` from `/api/doctor` hooks check; model-for-analyze dropdown (`GET /api/config` `models` + current) → `POST /api/config {model}`. **OMIT the API-key row** (retro doesn't manage keys; don't stub a security control).
- [ ] **Step 4:** **Projects** window (full width): table `PROJECT / OWNED FILES / TOKENS / ` + action. From `/api/xray` projects: name+path, owned files (CLAUDE.local.md/MEMORY.md present), tokens (sum of owned tokens_est), action = **Exclude** `.btn-danger`-ish via the veto-style confirm ("Stop watching <slug> and delete its retro-owned knowledge? Recoverable via store git history."). **No token-cap denominator, no pause/resume toggle** (no such feature) — column is just current tokens.
- [ ] **Step 5:** After any successful `POST /api/config`, re-fetch and reflect (and re-render dependent tabs' derived values like the held/live split on next visit). Show a transient "saved" affordance; on 503 (`run in progress`) show the message, don't lie about success.
- [ ] **Step 6: Verify** isolated: change threshold/budget/model → confirm `config.toml` on disk updated + reload persists; toggle theme → persists across reload; exclude a seeded project → gone. Paste outputs.
- [ ] **Step 7: Commit** — `feat(ui): Config tab — working threshold/budget/model/theme, real projects, honest omissions`.

---

### Task 7: Docs, scenario, 3.1.0

**Files:** `README.md`, `CLAUDE.md`, `scenarios/`, both `Cargo.toml`, `Cargo.lock`.

- [ ] **Step 1:** README Dashboard section + CLAUDE.md Plan-3 dashboard bullet: update to the desktop redesign (four tabs Overview/Knowledge/Activity/Config, light+dark, `POST /api/config`), and record the honest-omission list (decay/skins/token-cap/etc. are future work) so the gap is documented, not silently implied-present. Keep the `docs/design/retro-desktop/` handoff in the tree as the design reference.
- [ ] **Step 2:** Update the v3 dashboard scenario (`scenarios/v3-pipeline-dry-run.md` UI step) to curl `/api/config` (GET 200) and assert the served `/` contains the new chrome (`.menubar`, `front-panel`, `Overview`); keep the isolation preamble. Add a config-write assertion (`POST /api/config {confidence_threshold}` → GET reflects it) if the scenario harness can.
- [ ] **Step 3:** Bump both crates to `3.1.0` (retro-cli dep on retro-core → `3.1.0`); `cargo update --workspace` (stage Cargo.lock — sanctioned here); keep the JS `VERSION` const in sync with `3.1.0`. `cargo test` all green; `cargo build --release`; `retro --help` unchanged (eleven commands).
- [ ] **Step 4: Commit** — `docs+release: dashboard desktop redesign, 3.1.0`.

---

### Final: whole-branch review

- [ ] Dispatch a final reviewer over `origin/main...HEAD`: (1) **data-honesty audit — the priority**: grep the HTML for any hardcoded rule names/projects/numbers from the mockups (`payments-api`, `no-force-push-main`, `17,658`, `$0.41`, `.89 → .92`, `decayed`, `seen 7×`) that leaked in as fake data — every visible value must trace to an API field or be a static label; (2) company/employer-name scan across diff + messages (this is an open-source repo — no employer name anywhere); (3) XSS re-audit (all server strings through `esc()`, incl. the new tabs and modal); (4) API-contract check (every `fetch` hits a real endpoint with real fields; `/api/config` GET+POST correct); (5) the config-write endpoint's validation + run.lock; (6) `cargo test` (report count) + `cargo build --release` clean + isolated `retro ui` smoke of all four tabs; (7) confirm no `.hidden`/modal regression and theme persists. Fix findings, then push + PR (3.1.0), watch, and present merge.

## Out of scope (future plans)
- The features deliberately omitted for honesty: decay/aging, confidence-history tracking, per-run activity records (token deltas/candidates), weekly context-growth snapshots, AI-dollar-cost tracking, token caps, project pause/resume, API-key management, update-check, and the **skins system** (its own plan per `skins.md`). Each becomes a backend plan that later lights up the UI slot already designed for it.
