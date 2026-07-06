# CLAUDE.md — ocel-etl-backlog

Backlog connector: issues + comments + changeLog history → OCEL, with
incremental sync. Binary `ocel-backlog`, connector contract v1/v2. Concepts
in [ARCHITECTURE.md](ARCHITECTURE.md).

## Build, test, verify

```sh
cargo test          # paged/backoff transport is scripted — no network
cargo clippy --all-targets -- -D warnings && cargo fmt --check
BACKLOG_BASE_URL=… BACKLOG_API_KEY=… \
  cargo run --release -- pull --project KEY --out out.sqlite
```

After changing the binary: `cargo install --path .` (studio resolves it from
PATH). Real spaces/keys are private — never commit them; test data uses
`example.backlog.com`.

## Map

- `src/client.rs` — API client over an `HttpGet` transport abstraction:
  offset paging (issues), minId cursor (comments), 429 backoff honoring
  Retry-After / X-RateLimit-Reset
- `src/models.rs` — deserialization structs
- `src/mapper.rs` — `ProjectMapper`: task_created / comment_added +
  changeLog-derived status/assignee/priority/milestone/due_date events;
  streaming per issue (O(1 issue) memory); `skipped_fields()` counts what
  the whitelist drops
- `src/sync.rs` — incremental: prune by event-id prefix (`KEY/…`) → re-map
  updated issues → re-gate; parent-link repair
- `src/main.rs` — CLI (`pull`, multi `--project`, `--since`/`--full`,
  `--comment-bodies` opt-in), NDJSON progress

## Invariants and traps

- changeLog field names are `status` / `assigner` / `priority` /
  `milestone` / `limitDate` — not what the docs imply.
- Dynamic attribute initial values are reconstructed from the FIRST
  change's `originalValue`.
- Comment bodies are **opt-in** (`--comment-bodies`) — private data stays
  out by default (the GitHub connector is the opposite: public data,
  default on).
- Incremental correctness bar: equality with a full re-pull.
- Unknown changeLog fields are counted and reported, never silently
  dropped.

## Conventions

Issue → branch → PR → CI green → squash-merge. Unpublished (PATH binary).
Design docs live in the private ocel-workspace.
