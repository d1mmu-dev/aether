# Aether rebrand plan (before M7)

Owner decision (2026-11-23): the project becomes **Aether** before M7, and its full history later moves to the
existing `d1mmu-dev/aether` repository. This plan is separate from the M7 plan. Divergence row: **D-284**.

The rebrand is split into stages so that no running installation loses sessions, settings or data.

## Stage 1: UI brand and Midnight Slate theme (this slice)

Scope: what a user sees in `kima-web`. Nothing that is stored, sent over the wire or named in the build changes
in this stage (the storage-key guard test in `brand.rs` is removed by stage 2).

- Product name in the page title, header/sidebar mark, login, onboarding and footer copy: `Aether`
  (one constant, `kima-web` `brand::PRODUCT_NAME`).
- Owner-approved palette "Midnight Slate", replacing the amber/near-black upstream tokens:

  | Role | Value |
  |---|---|
  | page background (`--bg-primary`, Tailwind `black`) | `#0D1420` |
  | surfaces (`--bg-secondary/tertiary/hover`) | `#151F2E` (with small steps) |
  | borders | `#223047` |
  | accent (`--color-brand`, `--color-ambient`) | `#8BAFD1` |
  | primary text (`--text-primary`, Tailwind `white`) | `#DFE7EF` |
  | secondary text | `#9AAABA` |

- Changes live in the central tokens (`assets/kima-base.css`) plus the few hard-coded neutral surfaces in
  components. Mood, genre, decade, content-type and status (success/warning/danger) colours keep their meaning
  and stay as they are; album art is untouched.
- The logo mark becomes an own Aether glyph (SVG mask); the upstream mark asset stays in the tree (attribution
  in `assets/NOTICE.md`) until stage 2 decides its removal.
- Layout, fonts and controls are unchanged. The upstream colour parity of M9/M5b visual work is intentionally
  superseded for colours only (D-284); geometry parity stays the target.
- Gates: static brand tests (`kima-web --lib`), CSS/icon `--check`, nightly clippy native + wasm32, `vertigo build`
  into a fresh directory, real-WASM browser screenshots (desktop + phone) against a mocked API or the demo server.

Explicitly **not** in stage 1: crate/binary names, `KIMA_*` environment variables, `kima-web.*` localStorage
keys, the Subsonic `type`/`changedBy`/player values, database names, migration files, cookie/JWT claims,
data/config paths, Docker names, repository moves.

## Stage 2: technical rename (after stage 1, before M7)

Owner clarification (2026-11-23): the rebrand is **complete**, technical names included (crates, binary,
environment variables, storage keys, Subsonic identity, Docker, paths). The app is in development: the owner
accepts the loss of local development sessions/data where a rename makes it necessary and requires **no legacy
compatibility**. No permanent `KIMA_*` aliases and no storage compatibility layer are to be built. This is not a
wipe request: take a backup (`pg_dump -Fc`, data directory copy) before any destructive step.

1. **Never changed (byte-preserved):** all migration files and their checksums (including `HINT kima:profile_*`
   and the matching `HINT_*` constants), the HKDF info `b"kima-rs cipher v1"` (it keys existing encrypted
   settings), pinned goldens and model hashes, upstream GPL attribution, historical plans, ledger rows and the
   full Git history/provenance. The owner's music library, model weights, backups and unrelated services are
   not touched.
2. **Environment:** `Config::from_lookup` plus the ~10 direct variable readers move to `AETHER_*` in one
   step; docs, compose files and the private env files are updated together (backup first).
3. **Browser storage:** the ~98 `kima-web.*` references move to `aether-web.*`; users sign in once again.
4. **Subsonic identity:** `type`/`changedBy`/player values change only together with a real recapture of the
   522 snapshots (never manual edits).
5. **Crates, binary, Docker, paths:** renamed in one reviewed commit series; affected-scope gates per step.
6. **Repository:** full history goes to `d1mmu-dev/aether` only on the owner's explicit go; set and verify the
   local identity (`dimmu <3417783+d1mmu-dev@users.noreply.github.com>`) in that checkout before any commit, since
   it inherits a different global identity.
7. One complete local CI pass on the final technical-rename tip, then hosted CI, then the owner's merge decision.

## Stage 3: M7

M7 starts from the renamed tree; its plan does not carry rebrand work.
