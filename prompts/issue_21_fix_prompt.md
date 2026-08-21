# Task Prompt: Fix Issue #21 - Keep the old Solid square corners for every material

## Context
In repository `Hi9841/Prism`, `src-tauri/src/lib.rs` applies Acrylic, Mica, and Solid materials to the main borderless Tauri window. Tauri queues each native effect change on its Windows main-thread dispatcher, so Prism schedules its DWM corner preference after the effect operation.

## Problem Statement
PR #20 made all materials consistent by requesting `DWMWCP_ROUND`, but that selected the wrong baseline. The requested appearance is the old Solid behavior: square outer window corners. Acrylic and Mica restore Windows' rounded DWM shape when their backdrops are applied, while Solid clears the backdrop and previously appeared square. Prism 0.9.14 now forces rounded corners after all three transitions, producing visibly clipped outer corners and making every material differ from the requested Solid shape.

## Objective
Keep the outer Prism window square for Acrylic, Mica, and Solid by applying `DWMWCP_DONOTROUND` after every successful queued material transition. Preserve the existing material fallback and invalid-input behavior.

## Key Files to Modify
1. `src-tauri/src/lib.rs`
   - Replace the rounded-corner DWM preference with `DWMWCP_DONOTROUND`.
   - Rename the scheduling and application helpers so their names describe square corners.
   - Keep the corner operation ordered after Tauri's queued effect change and best-effort on unsupported systems.
   - Update the regression test to require the square-corner operation for Acrylic, Mica, Solid, and effect fallback behavior.

## Acceptance Criteria
- Acrylic, Mica, and Solid all request `DWMWCP_DONOTROUND` after their material transition.
- Switching between materials does not change the outer window corner shape.
- Effect fallback clearing still requests square corners.
- Invalid material values do not attempt, clear, or alter corner state.
- A real Windows window reports `DWMWA_WINDOW_CORNER_PREFERENCE = 1` after each of the three materials has painted.
- All existing tests pass (`cargo test --all-targets --all-features` and `bun run test`).
