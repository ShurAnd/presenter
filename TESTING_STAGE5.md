# Presentator 2.4.1 — Stage 5: video, audio and stage regression tests

Stage 5 adds **29 normal unit tests** on top of Stage 4 (95 passed / 21 ignored):

- `audio::flow_tests` — 5 tests of audio command transport and ordering, including load generations, two decks, pause/stop, seek/repeat/volume and channel shutdown.
- `video::flow_tests` — 4 tests of video command transport, relative/absolute seek, repeat and proper quit-on-drop, without media playback or a video window.
- `player::flow_tests` — 14 tests of playlist state, error handling, generation-based stale event filtering, stopping, clearing, removing and move boundaries, without libmpv.
- `commands::flow_tests::stage_*` — 6 tests of stage double-Esc, background/clock/timer and closing an active PDF presentation, without a GUI window.

No non-test production logic or dependencies are modified. Only `#[cfg(test)]` module declarations are appended to `audio.rs`, `player.rs`, `video.rs`; six stage tests are appended to the existing `commands_flow_tests.rs`.

Expected after extraction: **145 tests / 124 passed / 21 ignored**, assuming all new tests pass.

## Installation

Unpack `presentator_tests_stage5.zip` into `C:\rust\presenter` **with replacement of files**. This pack is incremental: Stage 4 **with the commands_flow_tests correction** must already be present. Alternatively apply `presentator_tests_stage5.patch` once; do not combine both methods.

## Verification

```powershell
cargo test flow_tests -- --nocapture
cargo test --all-targets
powershell.exe -NoProfile -ExecutionPolicy Bypass -File .\scripts\verify_tests.ps1 -Coverage
```

Do not use `cargo test -- --ignored` to run every ignored test: older end-to-end tests can interact with a real PowerPoint/show system.

## Important limitations

- Source packaging/diff can be verified here; Windows-only compilation and the test results must be verified on the user's Windows computer.
- Routing tests do **not** mean that the real libmpv, audio device, Media Foundation, video rendering, or multiple monitor setups are covered.
- Coverage may increase modestly: multimedia hardware code remains intentionally untested by default.
