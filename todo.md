# SCL3 Project TODO

## Milestone 0: Baseline and Validation
- [x] Confirm current app starts and UI can open `Launcher` and `Settings` pages.
- [ ] Add a short `README.md` at repo root with workspace/module overview and run instructions.
- [ ] Add basic `cargo check --workspace` to ensure all workspace members compile together.
- [ ] Decide target platforms for first release (Windows/Linux/macOS) and document scope.

## Milestone 1: Wire Real Launch Flow in `scl3`
- [ ] Replace placeholder `on_launch_game` callback in `scl3/src/main.rs` with actual launch pipeline.
- [ ] Build selected instance -> `scl_core::version::structs::VersionInfo` loading path.
- [ ] Resolve Java runtime from config (`game.java_path`) with fallback handling.
- [ ] Construct `scl_core::client::ClientConfig` from UI/config values.
- [ ] Call `Client::new(...).await` and `launch().await`, then show PID and status in UI.
- [ ] Surface launch errors in UI instead of only printing to stdout/stderr.

## Milestone 2: Implement Download/Install Actions
- [ ] Replace placeholder `on_open_download` behavior with real install/download entry workflow.
- [ ] Build a `Downloader` from config (`source`, `parallel_amount`, `verify_data`, `java_path`).
- [ ] Implement vanilla install flow using `VanillaDownloadExt`.
- [ ] Add optional loader install flow (Forge/Fabric/Quilt/NeoForge/Optifine) behind explicit UI choices.
- [ ] Persist installation choices per instance where applicable.
- [ ] Handle cancellation/retry logic for long-running downloads.

## Milestone 3: Progress and Async Integration
- [ ] Introduce a bridge from `scl_core::progress::Reporter` to Slint UI progress properties.
- [ ] Run long tasks off the UI thread and send progress updates safely back to UI.
- [ ] Show task state transitions: queued/running/success/failure.
- [ ] Ensure `download-task-name` and `download-progress` reflect real task status.
- [ ] Add guardrails for concurrent actions (prevent duplicate install/launch clicks).

## Milestone 4: Instance Management
- [ ] Replace static `instance-model` in `Launcher` page with dynamic data from disk.
- [ ] Define instance metadata format (name, version id, loader, game directory mode, last played).
- [ ] Implement create/rename/delete/select instance operations.
- [ ] Persist selected instance and restore at app startup.
- [ ] Map `instance-selected` callback to active instance context used by launch/download flows.

## Milestone 5: Account and Authentication
- [ ] Add account CRUD in settings/UI using `AuthConfig.accounts`.
- [ ] Integrate offline account creation using generated offline UUID.
- [ ] Integrate Microsoft login flow (client id + token refresh lifecycle).
- [ ] Integrate Authlib-Injector account login flow and credential storage.
- [ ] Store sensitive tokens/passwords only via keyring (`AccountConfig::save_secret/load_secret`).
- [ ] Add account selection on launcher page and bind to launch auth method.

## Milestone 6: Config and State Cleanup
- [ ] Remove or fix unused/typo module `scl3/src/gloabal.rs` (likely `global.rs`) and wire only if needed.
- [ ] Consolidate config read/write path logic (currently mixed Clapfig persistence + manual write).
- [ ] Validate config values before persisting (paths, memory bounds, source value).
- [ ] Add migration handling for future config schema changes.

## Milestone 7: UX and Error Handling
- [ ] Add visible status/error area in UI for launch/download/auth failures.
- [ ] Improve settings labels/help text consistency (CN/EN strategy based on language setting).
- [ ] Add disabled/loading states on buttons during active tasks.
- [ ] Keep page/index semantics consistent (`Pages` enum vs hardcoded index `10`).
- [ ] Review `ProgressBar` visibility logic and align with actual workflow pages.

## Milestone 8: Testing and Quality Gates
- [ ] Add unit tests for config mapping (`sync_config_to_ui` / `sync_ui_to_config` conversions).
- [ ] Add tests for version discovery and version type inference where feasible.
- [ ] Add tests for launch argument assembly critical paths in `deps/core/src/client.rs`.
- [ ] Add smoke tests for downloader source selection and path resolution logic.
- [ ] Add CI workflow for `cargo check`, `cargo test`, and formatting/lint checks.

## Milestone 9: Packaging and Release
- [ ] Define release profile/build matrix and artifact naming conventions.
- [ ] Add scripts/docs for packaging per platform.
- [ ] Verify runtime dependencies (Java requirement messaging, keyring backend expectations).
- [ ] Create release checklist (startup, install, launch, account login, rollback/retry behavior).

## Suggested Execution Order
1. Milestone 1 (real launch)
2. Milestone 2 + 3 (download + progress)
3. Milestone 4 + 5 (instance + account)
4. Milestone 6 + 7 (cleanup + UX hardening)
5. Milestone 8 + 9 (quality + release)
