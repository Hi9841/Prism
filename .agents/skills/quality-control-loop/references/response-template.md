# Response Template — quality-control-loop

Format your final answer as GitHub-flavored markdown. This becomes the PR body
and the `/iterate` workflow relies on it, so keep the structure intact.

```markdown
## Quality control: removed 1 panic-surface site

### Summary
- Metric before: `<N>` non-test panic-surface sites in `src-tauri/src`
- Metric after: `<N-1>`
- Sites removed this run: 1

### Change
- **File**: `src-tauri/src/<path>`
- **Line**: `<line>`
- **Before**: `<original panic-surface expression>`
- **After**: `<replacement expression>`
- **Pattern used**: `lock` | `parse` | `bare` — `<which golden pattern, e.g. lock().map_err(|e| e.to_string())?>`
- **Risk**: low (behavior-preserving)

### Validation
- [x] `bun run test`
- [x] `bun run build`
- [x] `cargo fmt --check`
- [x] `cargo clippy -- -D warnings`
- [x] `cargo test`
- [x] `bun scripts/sense-quality.ts` metric decreased by exactly 1

### Notes
<!-- Anything a reviewer should know; usually empty. -->
```

## Guidelines

1. Lead with the before/after metric so reviewers see scope immediately.
2. Quote the exact before/after expressions so the diff intent is obvious.
3. Mark every validation command as passed; if one failed, replace the `[x]`
   with `[ ]` and explain the blocker in Notes.
4. Keep it scannable — one change, one risk level, one pattern.
