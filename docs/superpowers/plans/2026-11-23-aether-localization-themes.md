# Aether localization and themes implementation plan

> **For Hermes:** Implement task-by-task with scoped tests and independent review at the completed-slice boundary. This document is planning only; do not interrupt the active technical-rebrand writer.

**Owner approval (2026-11-23):** Proceed in this order: finish full Aether technical rebrand and migrate the repository, then implement the localization/theme foundation, then subsequent M7/M10 screens on that foundation. Initial scope approved as proposed: Polish + English; Midnight Slate default with Graphite and Daylight options. Keep implementation phased and focused-test only. Do not introduce a second writer during the technical rename.

**Goal:** Offer translated UI and selectable, accessible themes without changing the established Aether layout or slowing the development loop with unrelated tests.

**Architecture:** Shared semantic CSS tokens define application colours; a validated theme identifier selects a bundled token set. A typed message catalogue supplies plain text and named arguments; locale-aware formatters handle plurals, dates and numbers. Preferences are personal to the active profile, with a device-level pre-login default and deterministic SSR behaviour.

**Tech stack:** Existing Rust/Vertigo WASM+SSR, Rust API and profile settings; existing CSS build tooling. Choose the smallest WASM-compatible Fluent/ICU implementation after a compatibility/build-size spike rather than assuming a new dependency works in Vertigo.

## Context and sequencing

- Owner approved full Aether rebrand and Midnight Slate as the default. Technical rename is currently running; do not edit its files concurrently.
- Localization and theme support are requested as planned work, not authorization to replace the rebrand task.
- Target repository after migration: the canonical Aether repository. Paths below use planned post-rename `aether-web`, `aether-api-types`, `aether-api`, `aether-db`; verify real paths before implementation.
- No current `src/i18n` module exists in the inspected frontend. Settings currently lives at `the pre-rename settings page` plus `pages/settings/`; rename will change the crate path.
- Recommend a short M-UX phase after the rebrand, before broad M10 page work. Introduce the message/theme infrastructure before new M7/M10 screens accumulate hardcoded strings, then translate existing pages in vertical slices. M7 backend need not wait for every translated page.
- Initial shipped languages proposed: English and Polish. German, Spanish and French are follow-ups only when actual translations can be reviewed; do not ship untranslated placeholder locales.

## Inspiration (retrieved 2026-11-23)

- Omarchy theme catalogue: https://learn.omacom.io/2/the-omarchy-manual/90/themes
- Tokyo Night token source: https://raw.githubusercontent.com/basecamp/omarchy/master/themes/tokyo-night/colors.toml (background #1a1b26, foreground #a9b1d6, accent #7aa2f7).
- Catppuccin token source: https://raw.githubusercontent.com/basecamp/omarchy/master/themes/catppuccin/colors.toml (background #1e1e2e, foreground #cdd6f4, accent #89b4fa).
- Borrow Omarchy's small named palette / coherent whole-app theme model, not its shell scripts or desktop integration. Verify the exact upstream revision, licence and attribution requirements before copying distributed palette assets. References above are mutable research URLs, not pinned implementation inputs.

## Proposed UX

Settings → Appearance & language:
- Language selector using native language names (`Polski`, `English`), not flags.
- Theme gallery with real palette swatches and a preview of a button, card and player. Midnight Slate is default; offer Graphite, Tokyo Night-inspired, Catppuccin-inspired, a light neutral theme and a high-contrast option after contrast review. Keep the initial release to Midnight Slate plus one dark alternative and one light alternative; additional presets are cheap only after the token audit is complete.
- Changes apply immediately without reload or interrupted playback. Persist successfully, show translated failure and roll back or clearly mark unsaved changes if persistence fails.
- Separate optional `System` appearance mode from a named theme: map OS light/dark preference to selected light/dark presets; a manual theme overrides OS changes. Do not treat every dark named palette as a distinct OS mode.
- Follow `prefers-reduced-motion`; no animated colour transitions for users who disable motion. Keyboard-focus, disabled/loading/error states and mobile controls must remain clear.

## Task 1 — Inventory and minimal foundation

**Files to inspect:** `crates/aether-web/src/{brand,style,shell,components,tracklist,player}.rs`, `src/pages/settings.rs`, `src/pages/settings/`, `assets/aether-base.css` (confirm actual renamed filename), API profile settings handler and database settings storage.

1. Inventory user-visible literals and hardcoded colour values programmatically; exclude artist/album/track metadata, user playlist names, upstream provenance and machine identifiers.
2. Record message keys by domain (common, auth, library, player, radio, mixes, podcasts, settings). Never use translated text as an identifier.
3. Add a short ADR for the catalogue/formatter choice after a small WASM+SSR compile and bundle-size check. Fluent is a candidate because plural forms and named variables are preferable to manually concatenated sentences; do not implement a new general-purpose localization framework.
4. Record current screenshots and focused playback behaviour as baseline; no full workspace pass.

## Task 2 — Locale core and catalogues

**Create:** `crates/aether-web/src/i18n/{mod,locale}.rs`, `crates/aether-web/locales/{en,pl}/<domain>.ftl` if Fluent wins the spike, otherwise document the selected catalogue extension before adding files.

1. RED: tests for locale validation, browser locale matching (`pl-PL`→`pl`, unsupported→`en`), missing-message fallback and named argument substitution; strings remain text nodes, not HTML.
2. GREEN: typed `Locale` and shared reactive application locale; no network fetch required for the two bundled catalogues.
3. Add independent plural tests for Polish (1,2,5,12,22,25,0) and English, including fractions where the formatter supports them. Handle complete translated sentences rather than concatenating translated noun fragments.
4. Format numbers, durations, dates and relative times through one locale-aware boundary. Locale and timezone remain separate; changing UI language must not change the mix-day timezone policy.
5. CI verifies catalogue key/argument parity, nonempty translations and invalid syntax. Missing keys fall back to English at runtime but fail catalogue checks in development.
6. Set document `lang`; add `dir` support to the locale model without claiming an RTL locale is accepted before its layouts are tested.

## Task 3 — Theme core

**Create:** `crates/aether-web/src/theme.rs`, `crates/aether-web/assets/themes/{midnight-slate,graphite,daylight}.css` (or one generated bundled stylesheet if that better fits the existing pipeline).

1. RED: tests reject unknown theme identifiers and verify every theme declares all required semantic tokens.
2. GREEN: validated theme enum, root `data-theme` and static bundled CSS. Tokens include background/surface/elevated surface/text/muted/border/accent/accent text/focus/selected/disabled/status colours.
3. Replace hardcoded interface colours in the inventory with semantic tokens. Do not recolour album covers; retain meaningful mood/content-category colours only if contrast is valid under each theme.
4. No user CSS, HTML, remote font or executable Omarchy theme packages. A later import feature, if requested, accepts only bounded validated palette data and has a separate security design.
5. Measure contrast for actual computed state colours (including alpha blending), targeting WCAG AA for text and visible focus/non-text controls. Include muted/disabled distinction without promising disabled text must meet ordinary text thresholds.

## Task 4 — Persist personal preferences safely

**Modify:** existing profile settings DTO, validated API route and storage; inspect actual handler paths before coding. Prefer existing settings JSON/kv fields over a new SQL migration where its ownership and validation fit.

1. RED: invalid locale/theme values rejected; save/read round trip; profile A cannot change profile B; switching profiles updates both preferences.
2. Store only enum-like identifiers, not arbitrary CSS/paths or source markup. Do not log tokens or private settings.
3. A browser-local, non-sensitive pre-login default selects the login shell. Server SSR cannot read localStorage: render a deterministic default shell, apply stored visual preference before first paint using a small CSP-compatible bootstrap if feasible, then apply authenticated profile preferences. Never claim SSR magically renders local browser preferences.
4. Define precedence: authenticated profile choice → remembered device/pre-login choice → browser locale/OS mode → English/Midnight Slate fallback. Avoid global SSR state leaking one profile's preferences to another.
5. Verify no session/auth storage format or playback queue changes are needed. Cross-tab adoption must follow the existing session/profile contract, not invent shared account-wide preference changes.

## Task 5 — First vertical slice

**Modify:** settings, shell/navigation, login/onboarding, shared dialogs/toasts and common player labels.

1. Translate complete labels, placeholders, validation and accessibility text (`aria-label`, titles, buttons, loading/errors, confirmations).
2. Add Settings selectors and preview gallery; test switching both locale and theme while a real track plays without reloading, pausing or resetting position.
3. Preserve raw server diagnostics for troubleshooting but present localized messages for recognized UI errors. Do not rewrite the public REST API error contract merely to translate the UI; unknown errors use a safe localized fallback.
4. Browser checks: English/Polish × dark/light at desktop/mobile; keyboard focus, long Polish labels and login shell before authentication.

## Task 6 — Remaining screens in independent slices

Translate library/search/playlists → radio/mixes/vibe → podcasts → advanced settings/admin. New M7/M10 UI must use message keys from introduction. Metadata and user content remain unchanged. Track coverage by user-facing messages/pages, not line count; do not announce full localization while strings remain hardcoded in dialogs or errors.

## Focused verification and cost controls

- Catalogue-only changes: catalogue syntax/key/placeholder tests, not full backend/ML tests.
- Theme CSS-only changes: token/contrast checks, CSS `--check` and scoped screenshots; reuse unchanged WASM.
- Frontend infrastructure changes: `cargo test -p aether-web --lib`, focused locale/theme tests, incremental WASM build and relevant browser flows.
- Profile preference persistence: only affected API/settings/isolation tests on identity-checked isolated DB; never owner application DB.
- Once the initial shared locale/theme foundation is complete: one integration regression gate for auth/profile switching/player. Do not repeat a full workspace pass after every translation file or preset.
- All browser audio through the dedicated silent sink and watchdog. Trusted clicks under standard autoplay for playback-preservation acceptance.
- Report compiler/link time separately from test execution and catalogue checks.

## Acceptance criteria

- English and Polish cover the first agreed screen slice with no raw keys/missing placeholders.
- Profile and pre-login locale/theme precedence is tested and documented.
- Midnight Slate remains default; alternate themes are readable, keyboard-accessible and responsive.
- Switching language/theme does not reload, stop audio or lose the active profile.
- No flash of the wrong theme in the tested initial-load path, or a precise documented remaining SSR/CSP limitation rather than a false pass.
- No palette/theme code can execute imported scripts or CSS from untrusted origins.
- Legal provenance verified before redistributing borrowed presets.

## Open choices (not blocking planning)

1. Confirm launch language order: recommend Polish + English first; then German/Spanish/French according to actual demand and reviewer availability.
2. Confirm whether preferences should be per profile as proposed; do not migrate to account-wide settings without an explicit decision.
3. Pick first two optional presets from real previews. Recommended initial set: Midnight Slate, Graphite and Daylight; Tokyo Night/Catppuccin-inspired additions follow licence/contrast review.

## Risks and trade-offs

Global colour-token changes are a small code diff with broad visual effects; verify shared surfaces, not only the selected Settings page. SSR and browser locale resolution can diverge; keep deterministic fallbacks and no cross-request mutable locale. Polish pluralization requires real language rules. Bundling many locales grows WASM/download size; measure before adding lazy-loading infrastructure. Translation reviews and palette accessibility matter more than the number of advertised languages/themes.

## Stage ledger

Divergence rows reserved for this plan: **D-286..D-289** (after the live ledger's D-285). D-286 is used by stage 1. D-290 (outside the reservation) records the radio performance extension below.

### Stage 1 (2026-11-23, branch `feature/appearance-i18n`) — foundation + first Settings slice, local only

Done:
- Catalogue decision (task 1/ADR): typed Rust messages (`src/i18n.rs`): `Locale {En, Pl}`, `Msg` enum with both texts per arm (key parity by construction), Polish plural rule (`pl_plural`, tests 0/1/2/4/5/12/14/21/22/25/101/112/122) and a whole-sentence `track_count`. No dependency, no runtime catalogue loading. Fluent was not added: this slice needs plain labels and one plural rule; Fluent stays the candidate when translator-editable files or many named arguments are needed (re-evaluate then with a WASM size check).
- Theme core (task 3): `src/theme.rs` validated `Theme {MidnightSlate (default), Graphite, Daylight}`; `assets/themes.css` plain CSS appended unlayered after the compiled stylesheet (no Tailwind regeneration), each theme re-declares all 19 semantic tokens the utilities read. Tests: unknown ids rejected, every theme declares every token, no `@import`/`url(`/script, WCAG AA (4.5:1) for primary/secondary/muted/brand/danger text on the ground, a `white/10` overlay (sRGB alpha blend) and the tertiary surface; on-brand 4.5:1; focus ring 3:1.
- Preferences (task 4, partial): `src/prefs.rs` reactive locale/theme bound to `<html lang>` and `<html data-theme>`; **device-level only** (`localStorage` keys `aether-web.locale` / `aether-web.theme`, enum ids only, tampered values fall back). Precedence implemented: device choice -> `navigator.language` -> English / Midnight Slate.
- Settings -> Appearance & language (task 5, partial): language buttons (native names) and theme buttons with swatches, `aria-pressed`; Settings header, section nav, section titles and the desktop rail + phone bottom navigation are translated.
- Verified: `aether-web` lib tests 201/0 (new i18n/theme/prefs tests included), clippy (lib + tests) clean, fmt, `vertigo build` OK; DB-free headless browser smoke on the real WASM (`vertigo serve`, no API, so wiring only, not server acceptance): SSR default `lang=en data-theme=midnight-slate`, immediate switch to Polish + Daylight without reload (computed body background/colour change), Graphite applies, choice stored, login shell picks up stored device prefs after hydration.

Stage 1 review fix (Important, Daylight Vibe unreadable):
- Design decision: the Vibe map is a **fixed dark night scene** in every theme. Its sky/glass surfaces, white point outlines and inline mood-chip colours are dark-only, so the scene root (`.vibe-sky`) shares the Midnight Slate token rule in `assets/themes.css` (plus `color: var(--text-primary)` for uncoloured text). Inside the map all `text-white`/`--text-*`/brand utilities resolve to Midnight Slate; the shell, sidebar and mini player keep the selected theme. Daylight is not disabled.
- `<meta name="theme-color">` now follows the selected theme's ground (`Theme::swatch().0`).
- Verified: RED->GREEN test `vibe_scene_text_meets_wcag_aa_on_sky_and_glass_in_every_theme` (Daylight `text-white` on the sky was 1.02:1); `aether-web` lib tests 203/0; CSS `--check` up to date; `vertigo build`; DB-free browser smoke on the real WASM with a CDP-mocked `/api/*` fixture (120-track map, sound search) in Midnight Slate and Daylight: 11/11, scene text 14.6:1 (primary) and 5.9:1 (`white/60`) on glass in both themes. Not server acceptance.

Task 4 (per-profile persistence, D-287):
- Storage: migration 0066 nullable `user_settings.ui_locale`/`ui_theme` with CHECK constraints (the settings table has no JSON field to reuse); API `GET/POST /api/settings` `uiLocale`/`uiTheme` (null = not chosen / clears; invalid -> 400 with details, nothing written).
- Web: `Prefs` profile layer driven by the session token (subscription outside render, deferred writes), generation fence + the client's token fence; precedence profile -> device -> browser -> defaults; signed-in choice saved to the profile only, failed save rolled back with a localized toast; the Settings note now says the choice is saved to the profile.
- Verified (scoped): `aether-api --test settings` 39/0 incl. 3 new (round trip/partial/clear, invalid+unknown values 400 without writes + DB CHECK, profiles A-main/A-kids/A-standard/B isolation) on the identity-checked isolated cluster; `aether-db --test user_settings` 4/0; `aether-api-types` profiles_wire/web_pages 3+6/0; `aether-web` lib 208/0 (5 new prefs tests: precedence, switch never reuses the previous profile, late old answers ignored, sign-out keeps the device choice, rollback only of its own choice/session); clippy (changed crates) clean; fmt; CSS `--check` up to date; `vertigo build`; DB-free browser smoke on the real WASM with a CDP-mocked `/api/settings` 13/13 (fallback while loading, profile choice applied, immediate save without reload, failed save rollback + toast, next profile never shows the previous look, sign-out keeps the device choice), silent sink, nothing played.
- Abyss (owner request 2026-11-23, custom Aether theme, not an imported package): `Theme::Abyss` id `abyss` (ground #070C12, surfaces #0D1722/#142331, hover #1B2D3D, accent #80BCC8, text #E1EBF1/#A5B9C7/#839BAE, borders/brand variants derived), full token block in `assets/themes.css`; Settings selector now enumerates `Theme::ALL`/`Locale::ALL`; ids centralized in `aether_api_types::settings::UI_THEMES`/`UI_LOCALES` (API validation, DB CHECK in 0066, web tests assert equality). Verified: existing WCAG surface/overlay/Vibe-scene contrast tests pass for Abyss; `aether-web` lib 210/0; `aether-api --test settings` 39/0 (every shared theme id saved through route + CHECK); browser on the real WASM with the mocked API 3/3 (applied, ground rgb(7,12,18), four choices in order, save) and the profile smoke 13/13 again. Midnight Slate stays the default.
- Not done: real-server run (pending), in-document profile switch race only unit-tested (the real switch replaces the document), first-paint bootstrap.

Task 5, first vertical slice (header, sign-in, first run, player):
- Catalogue: `catalogue!` macro declares each key with both texts and generates the test-only `Msg::ALL`, so a key cannot be missed by the tests; 85 keys. Sentences with arguments are functions per locale (`listening_as`, `sign_in_to`, `owner_account_intro`, `scanning_dir`, `scan_failed`, `added_to_queue`, `sleep_timer_left`, `sleep_preset` with the Polish plural); tooltips use `Msg::hint` (the legacy lower-case `title=`).
- Reactive: rendered labels are `Prefs::text/hint/format` computeds (follow an in-place language change); one-off toasts take the current language (`Prefs::now/current`). The player bar takes the locale as a `Computed<Locale>` argument; no player state/audio path changed.
- Errors: form validation returns `Msg`; a form shows `UiError::{Msg, Server}`. Only the sign-in 401 is mapped (`UiError::sign_in`) to a localized message; every other server body is shown as received; REST contract untouched. English sleep preset 90 now reads "90 minutes" (was "1 hours").
- Translated: desktop/phone header (search placeholder/label, home, sync, settings, menu buttons), profile menu (switch, listening as, who's listening, manage, sign out), login, first-run setup (labels, placeholders, validation), player bar and mobile mini player labels/tooltips/aria (transport, repeat states, shuffle station hint, seek, mute, volume, sleep timer and presets), add-to-queue toast/label, scan toasts, empty-library shuffle toast.
- Verified (scoped): `aether-web` lib 222/0 (5 new catalogue tests), clippy lib+tests clean, fmt, `vertigo build` (0.14.5); browser on the real WASM with a CDP-mocked API, trusted clicks after hydration, silent sink, nothing played: 13/13 (SSR English default, login/setup Polish incl. validation and 401, shell/player Polish from the profile choice, in-place switch to English and back without reload, phone header). Not server acceptance.

Task 5, second slice (Settings account/profiles/apps, footer):
- Catalogue 157 keys (+72). New sentence functions: `profile_count`, `type_name_to_confirm` (profile name verbatim), `token_dates` (timestamps verbatim, "never"/"nigdy"); `brand::footer(locale)` keeps "© 2026 Aether." fixed and translates only the tagline.
- Client validation returns `Msg` (password: current/length/match; profile: name length, kind, avatar; app token: device name required/too long); forms show `UiError`, so a shown error follows an in-place language change. Server bodies (e.g. a wrong current password) stay verbatim. The change-password success (200) shows the catalogue text instead of the server's English body; the token pair replacement is unchanged.
- Translated: account rows (signed in as, Administrator/Member, password row and form, podcast AI switch), profiles (intro, counter, kind labels Main/Kids/Standard/Unknown, editor, avatar names, limit/permission notes, typed-name deletion warning/prompt/button, toasts), apps (intro, empty state, device name field, create/refresh/revoke, one-time secret panel, copy/blocked/dismiss, row dates, toasts), login/setup footer tagline. Guards unchanged: main-only password/profile management, typed exact-name deletion, busy serialization, one-time secret page-local.
- Verified (scoped): `aether-web` lib 227/0, clippy lib+tests clean, fmt, CSS `--check` up to date, `vertigo build` 0.14.5; browser on the real WASM with a CDP-mocked API (synthetic tokens, secret never logged/stored), trusted clicks, silent sink: 26/26 (footer, account/password validation + verbatim server error + localized success, profiles + deletion guard, in-place switch EN and back, apps secret/copy/dismiss/revoke, phone without horizontal overflow). Not server acceptance.

Task 6, first slice (collection, search, playlists + shared labels):
- Catalogue 220 keys (+63). Count sentences with the three Polish forms: `track_count`, `song_count`, `artist_count` (artysta/artystów/artystów), `album_count`, `playlist_count`; `page_of`/`page_range` (pager, "Strona 2 z 3", "1–40 z 120"), `total_duration` ("1 godz. 5 min"), `play_named`, `delete_playlist_confirm`, `added_to_playlist` (names verbatim in „…”).
- Shared pieces follow the language in place: `ui::resource_view` (now takes `&Prefs`; `Loading…` for every caller, including podcasts/settings), `pager`, `empty_state`/`section_header`/`eyebrow` (accept `Computed` text), `media_card` (play label; `Card.subtitle` is a `Computed`, `ui::fixed_text` for metadata), collection masthead/tabs/Refine panel (sort labels are `Msg`, the `sortBy` ids unchanged), track-row headers and actions, add-to-playlist picker and toasts, desktop sidebar (navigation index, playlist stack + counts, main-navigation label, also on the phone bottom bar).
- Pages: collection artists/albums/tracks (counts, empty states, toasts), search (hint, sections, no matches; the query heading is the user's text), playlists list (create form; `validate_name` returns `Msg`, errors are `UiError` so they follow a language change) and detail (counts/duration, play/shuffle/visibility/delete/remove labels, delete dialog). API errors, library/user names, search queries, request URLs/bodies unchanged; permissions/ownership/visibility logic unchanged. Pager position is one sentence per language (the page numbers are no longer separately bold).
- Verified (scoped): `aether-web` lib 229/0, clippy lib+tests clean, fmt, CSS `--check` up to date, `vertigo build` 0.14.5; browser on the real WASM with a CDP-mocked API, trusted clicks, silent sink, nothing played: 33/33 — in-place EN->PL while the profile's `uiLocale` arrives (loading row, open Refine panel, open delete dialog + client validation error keep state), PL counts/pager/sort request ids, empty/error states, add-to-playlist toasts, search (encoded query, verbatim heading), create/visibility/delete with unchanged bodies (exact write audit), Settings switch to EN updates the sidebar in place and pages after SPA navigation, phone layouts without horizontal overflow. Not server acceptance.
- Noted, not changed (pre-existing): in the collection track table the add-to-playlist picker of a row is covered by the next row (rows' animation creates stacking contexts); only the last row's picker is fully clickable. The Refine choice is shared across the three tabs.

Task 6, second slice (radio, mixes, Vibe + remaining shell labels):
- Catalogue 309 keys (+89). Sentences: `track_total`/`song_total` (locale digit grouping: `1,234` / `12 345` from five digits), `station_count`/`genre_count`/`decade_count` (three Polish forms), `radio_description`, `decade_name` ("80s"/"lata 80."), `station_no_tracks`/`station_too_few`, `sound_rebuild_line` + `mood_stations_note` (the note stores the `SoundRebuild` step and is written when shown), `mix_duration` (moved from `api::mixes::total_duration`), `mix_save_failed`, `play_mix`, `add_named_to_queue`, `vibe_name_matches`, `sound_search_failed`, `similar_tracks`, `vibe_queued`.
- Radio: station tiles carry a `Label` (`Fixed(Msg)` quick stations, `Server` mood/genre names as received, `Tracks(n)`, `Decade(y)`); request `type`/`value`, station cursor/seed, refill and loop generations unchanged. Mixes: row, mix page, 409/refresh/missing states and toasts; mix names/descriptions and server errors verbatim. Vibe: notes, search box, name/sound summaries, legend (`logic::mood_label` returns `Msg`; mood keys, colours, projection, lit layers unchanged), hint, track panel, similar list; `MapLoad::Failed`/`Matches::Failed` hold `UiError` so own messages follow the language. Shell: skip link, queue panel eyebrow/empty/collapse, phone drawer quick links/actions/logout, the player's match-this-vibe button.
- Verified (scoped): `aether-web` lib 230/0, clippy lib+tests clean, fmt, CSS `--check` (regenerated: the removed "collapse panel" literal dropped an unused `.collapse` utility), `vertigo build` 0.14.5; browser on the real WASM with a CDP-mocked API built from the captured M5b fixtures, trusted clicks, silent sink, nothing played: 19/19 — in-place EN->PL on radio (incl. the rebuild note), mix page and Vibe with the track panel open; station start requests keep `type=discovery` / `type=mood&value=calm` and end in Polish too-few/failure toasts; mix refresh failure, save 409 toast + navigation, missing mix; Vibe search body as typed, 503 sound-off note, similar list; phone radio/mix/vibe without horizontal overflow and the drawer; exact write audit. Not server acceptance. The first browser run failed on three harness selectors only (`smoke-run1-harness-selectors.log` in the private evidence).

Task 6, third slice (podcasts + final inventory of the existing web UI):
- Catalogue 366 keys (+57). Sentences: `found_new_episodes` (from the typed `newEpisodesCount`: "Znaleziono 1 nowy odcinek / 3 nowe odcinki / 12 nowych odcinków"), `queued_podcasts` (typed `queued`: podcast/podcasty/podcastów), `ai_hidden_items` (pozycję/pozycje/pozycji oznaczoną/-e/-ych jako AI), `percent_played`, `because_you_listen`, `refresh_podcasts_failed`.
- Podcasts home/search/category/show/preview: headings, sections, empty states, feed form + client validation (`UiError`, follows the language while shown), search box/results, pill, hidden counts, follow/following, refresh buttons and toasts (from typed counts; other server texts verbatim), episode meta/order/range/pager, play labels, admin label buttons, AI/Human/Unlabelled badges with source tooltips. Show/episode titles, authors, descriptions (HTML->text), Apple category names, for-you reasons' names, feed URLs and server error bodies stay as received. API ids/values (`ai=hide|show`, `order=oldest`, category/offset/limit, subscribe/label bodies), the default-hidden AI filtering and episode playback/resume/progress/queue code are unchanged. AI-content labels describe how content was made; no text equates them with age suitability (kids profiles keep their separate kind label). Podcast data has no explicit-content flag, so none is shown.
- Stragglers fixed: unknown-address page and the profile picker (Kids badge, manage profiles, who's listening, sign out).
- Inventory (`inventory.py` heuristic over all non-test `aether-web` literals + classified residue): no remaining untranslated interface label found in existing pages. Remaining English by design or later: the client fallback "Request failed (status)" for unreadable error bodies (plan later), server error bodies/toasts shown as received, SSR first paint, theme preset and brand names, codec diagnostics. There are no separate advanced/admin settings screens in aether-web to translate.
- Verified (scoped): `aether-web` lib 235/0, clippy lib+tests clean, fmt, CSS `--check` up to date, `vertigo build` 0.14.5; browser on the real WASM with a CDP-mocked API from the captured podcast fixtures, trusted clicks, silent sink, nothing played: 18/18 — in-place EN->PL on the home page (incl. a displayed feed validation error) and the show page; search queries, pill refetch with `ai=show` and no settings write, refresh-all and show refresh toasts, category paging requests, order flip, admin label body, unfollow (DELETE, back to /podcasts without reload), preview follow body + navigation, 404 and profile picker in Polish, phone layouts without horizontal overflow, exact write audit, no console errors. Not server acceptance. Run 1: 17/18, harness timing only (checked before the pill's refetch was logged).

Bounded performance extension (owner approval 2026-11-23, "stations at once, covers in the background", D-290):
- Problem: after a restart the in-memory cover store is cold and `GET /library/radio/row` works out one radio selection per station before answering, so `/radio` showed only placeholders for seconds.
- API: `?covers=none` returns the same stations/order/keys with `cover: null` and picks or keeps no cover (zone still kept for the warm job); other values or a repeated key 400 `covers must be none`; the default reply is unchanged. No new cache, worker or spawn; the existing warm job is untouched.
- Web: tiles from the `covers=none` reply (playable at once); one request for the unchanged full row lends covers to the shown tiles, matched by type/value (stations, counts and order stay from the first reply); a covers failure keeps the placeholders, no error, no retry; late answers fenced by the page's visit generation and, on profile switch, by the replaced document/retired client. Station start, seed/cursor/refill and shuffle rule unchanged.
- Verified (scoped, local): `aether-api --test radio_row` 14/14 (2 new, RED->GREEN), `aether-web` lib 238/0 (3 new), clippy native/wasm32/API, fmt, CSS `--check`, `vertigo build`; real-WASM browser with a CDP-mocked API and a held/failed/delayed full row: 10/10 (base build RED: no tiles while the full row is held). Simulated 3 s cover delay: tiles ready about 0.11 s vs about 3.1 s before; covers unchanged at about 3.1 s (mock measurement, not the owner's server).
- Not claimed: real-server timing, independent review, hosted CI for these commits.

Remaining (not claimed):
- First paint: SSR always renders the default; a stored non-default theme/language applies after hydration (possible flash). A CSP-compatible bootstrap is not done.
- Task 5 remainder: none found by the inventory in existing dialogs/toasts (server-provided texts stay verbatim). Some status/type colours are not themed yet.
- Task 2 remainder: number/date/duration formatters; catalogue CI beyond the unit tests.
- Playback-preservation check while switching (real track, trusted clicks, silent sink), mobile/desktop screenshots EN/PL x dark/light, independent review.
- Task 6: existing pages translated (see the third slice inventory); open: the client "Request failed (status)" fallback and status-classified known server errors at the presentation boundary. SSR still renders English first (`web_ssr_embed` asserts English texts; not rerun in this slice).
- Deferred minors (unchanged, not fixed in translation slices): add-to-playlist picker covered by the next row; Refine choice shared across tabs.
