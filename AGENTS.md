# AGENTS.md

Repo convention: comments, commit messages, README, and installer language are **Chinese (zh-CN)**. Match that when adding user-facing strings or commit messages.

## What this is

Tauri 2 desktop app (Windows-first): a floating-subtitle overlay tool whose subtitle windows are meant to be captured by OBS. Frontend React 19 + TS + Vite 7 + Tailwind 4; backend Rust with SQLite (`rusqlite` `bundled`). Single crate + single frontend, no workspace, no monorepo.

## Commands

```bash
npm install            # once
npm run tauri dev      # dev: Vite on :1420 + cargo rebuild on Rust change
npm run tauri build    # release build (plain exe + NSIS in src-tauri/target/release/bundle/)
npm run bundle         # tauri build --bundles nsis,msi  (MSI is best-effort, often fails on CI — see below)
npm run bundle:portable # build exe + node scripts/package-portable.mjs -> dist-portable/
npm run bump -- 0.0.5  # bump root package.json version + sync package-lock; no commit, no tag
```

- There are **no tests, no lint, no formatter config** in this repo. `npm run build` runs `tsc` (strict, `noUnusedLocals`/`noUnusedParameters`) and is the only typecheck gate.
- Vite dev server is pinned to port **1420** with `strictPort` — must be free, `tauri.conf.json` `devUrl` depends on it. `clearScreen: false` is intentional so Rust compiler errors stay visible; don't change it.

## Version number

`package.json` `version` is the **single source of truth**:
- `src-tauri/tauri.conf.json` has `"version": "../package.json"` (installer/exe version follows).
- Frontend reads it via Vite `define.__APP_VERSION__`.
- `src-tauri/Cargo.toml` version is a placeholder `0.0.0` — leave it alone.

Release flow: `npm run bump -- X.Y.Z` → commit → `git tag -a vX.Y.Z` → push tag. Tags `v*` trigger the release workflows.

## Release pipelines (two, for different hosts)

- `.github/workflows/main.yml` — Windows runner; NSIS installer + portable zip; MSI built `continue-on-error` because WiX `light.exe` is often blocked by Defender on runners. Main release path.
- `.cnb.yml` (CNB, Linux container) + `.workflow/打包.yml` — Linux can't produce NSIS, so it cross-compiles the exe with `--target x86_64-pc-windows-gnu --no-bundle` and **must copy `WebView2Loader.dll`** (searched from the cargo registry or `target/`, x64 only) next to the exe before `package-portable.mjs`. Don't "simplify" that step away.
- `bundle.targets` in `tauri.conf.json` stays `["nsis"]`; MSI is only produced by overriding with `--bundles msi` on the CLI.

## Architecture notes

- **Single source of truth is Rust's SQLite.** Every UI edit goes `invoke` command → SQLite commit → Rust broadcasts the full config via `emit("config-changed")` → every window re-renders. Don't cache/mutate config locally in the frontend beyond optimistic state; a change not written through a command will be lost on next broadcast.
- **Window routing by label** in `src/App.tsx`: label `main` → main shell; label `overlay-{id}` → `OverlayWindow`. New overlay windows are created from Rust (`overlay_win.rs`) with that label scheme; capabilities (`src-tauri/capabilities/default.json`) grant permissions to `["main", "overlay-*"]` — new Tauri APIs need a permission entry here or they fail at runtime.
- **State registration order matters**: `app.manage(store)` must happen before any window is created, or the overlay's first `get_config` call errors with "state not managed". See `src-tauri/src/lib.rs`.
- Data dir: installed → `%APPDATA%/com.izuna.toolbox/`; portable (a `portable.flag` file next to the exe) → `data/` beside the exe. `PORTABLE_FLAG` lives in `lib.rs`.
- Auto-update is Windows-only (`cfg(windows)` gate on `reqwest`/`tokio` deps in `Cargo.toml`); the download runs on the Rust side because release-asset CDN redirects block cross-origin reads from the webview.
- Initial/seed data (default overlay, variables, computed vars) is defined at the top of `src-tauri/src/store.rs` (`SEED_VARIABLES` / `SEED_COMPUTED`) — used for both first-run and re-seeding an emptied overlay.

## Size-conscious build profile

The project deliberately minimizes binary size: `rusqlite` bundled, `webviewInstallMode: skip`, release profile `lto + strip + opt-level "s" + panic abort`. Avoid adding heavyweight deps or re-enabling the WebView2 bootstrapper unless the user asks.

## Reference

`README.md` (Chinese) documents the feature set, data model, and update flow in detail and is kept current — consult it before guessing at behavior.
