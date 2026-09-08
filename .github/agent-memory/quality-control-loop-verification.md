# Quality-control loop — local verification

Run date: 2026-09-08T16:17:05Z (UTC)

## Sensor — `bun scripts/sense-quality.ts`

```json
{
  "schema": "prism-quality-loop-sensor/v1",
  "generatedAt": "2026-09-08T16:17:05.555Z",
  "scope": "src-tauri/src",
  "metric": 25,
  "counts": {
    "total": 381,
    "nonTest": 25,
    "testOnly": 356,
    "byKind": {
      "unwrap": 23,
      "expect": 2
    },
    "byCategory": {
      "lock": 15,
      "parse": 8,
      "expect": 2
    }
  },
  "sites": [
    {
      "file": "src-tauri/src/catalog/mod.rs",
      "line": 369,
      "column": 13,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/mod.rs",
      "line": 470,
      "column": 17,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/mod.rs",
      "line": 687,
      "column": 17,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/mod.rs",
      "line": 769,
      "column": 21,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/mod.rs",
      "line": 774,
      "column": 21,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 229,
      "column": 42,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "let name_length = read_u16(bytes, 56).unwrap() as usize;"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 230,
      "column": 42,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "let name_offset = read_u16(bytes, 58).unwrap() as usize;"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 250,
      "column": 36,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "frn: read_u64(bytes, 8).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 251,
      "column": 44,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "parent_frn: read_u64(bytes, 16).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 252,
      "column": 37,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "usn: read_i64(bytes, 24).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 253,
      "column": 43,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "timestamp: read_i64(bytes, 32).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 254,
      "column": 40,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "reason: read_u32(bytes, 40).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/ntfs/usn.rs",
      "line": 255,
      "column": 44,
      "kind": "unwrap",
      "category": "parse",
      "snippet": "attributes: read_u32(bytes, 52).unwrap(),"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 265,
      "column": 31,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "shard_roots.lock().unwrap().insert(shard.root);"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 266,
      "column": 27,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "handles.lock().unwrap().push(reader_handle);"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 296,
      "column": 23,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "handles.lock().unwrap().push(flusher_handle);"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 324,
      "column": 42,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "for handle in self.handles.lock().unwrap().iter() {"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 370,
      "column": 36,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "self.shard_roots.lock().unwrap().insert(shard.root.clone());"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 371,
      "column": 32,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "self.handles.lock().unwrap().push(handle);"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 475,
      "column": 21,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 509,
      "column": 21,
      "kind": "unwrap",
      "category": "lock",
      "snippet": ".unwrap()"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 563,
      "column": 43,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "event_queue.lock().unwrap().push(event);"
    },
    {
      "file": "src-tauri/src/catalog/watcher.rs",
      "line": 602,
      "column": 39,
      "kind": "unwrap",
      "category": "lock",
      "snippet": "let mut q = event_queue.lock().unwrap();"
    },
    {
      "file": "src-tauri/src/lib.rs",
      "line": 274,
      "column": 9,
      "kind": "expect",
      "category": "expect",
      "snippet": ".expect(\"failed to run Prism\");"
    },
    {
      "file": "src-tauri/src/power.rs",
      "line": 130,
      "column": 9,
      "kind": "expect",
      "category": "expect",
      "snippet": ".expect(\"sleep has suspend parameters\");"
    }
  ]
}
```

## Controller — `bun scripts/control-quality.ts .qc-sensor.json --feedback .github/agent-memory/quality-control-loop.md`

```markdown
# Quality-control target

- **Metric before this run**: 25 non-test panic-surface sites in `src-tauri/src`
- **Batch size**: 1 (selected 1)
- **Exclusions applied**: 0

## Target 1

- **File**: `src-tauri/src/catalog/mod.rs`
- **Line**: 369
- **Kind**: `unwrap` · **Category**: `lock`
- **Snippet**: `.unwrap()`
- **Risk**: low (behavior-preserving; one site)

## Guidance

Read and follow the `quality-control-loop` skill (`.agents/skills/quality-control-loop/SKILL.md`).
Fix only the selected site(s), preserve behavior, and keep every validation command green
before committing. Format the final answer per the skill's `references/response-template.md`.

```

## Workflow YAML validation

Validated with `bunx js-yaml .github/workflows/quality-control-loop.yml` — parses without error.
