# Architecture

How the Backlog connector turns issue history into OCEL.

## Events from the changeLog, not from snapshots

Backlog's REST API exposes each issue's changeLog; the mapper turns
whitelisted field changes (status, assignee, priority, milestone, due date)
into timestamped events, alongside task_created and comment_added. Initial
attribute values are reconstructed from the first change's `originalValue`,
so the log carries the full lifecycle even for fields that changed before
extraction. Whatever the whitelist drops is counted and reported — nothing
disappears silently.

## Faithful extraction, interpretation downstream

The connector's only options are privacy (comment bodies are opt-in) and
scope (projects, since). It does not clean, dedupe, or reinterpret — that is
the recipe layer's job on a separate output file, so the raw pull stays
recoverable.

## Streaming and incremental

Issues stream one at a time (comments are mapped and dropped per issue), so
memory is O(1 issue). Incremental sync prunes the key-prefixed slice of a
previous output, re-pulls only issues updated since the newest event, and
re-gates through ocel-etl's validation; the correctness bar is byte-for-byte
equality with a full re-pull.

## Tested without a network

The API client sits behind an `HttpGet` transport trait; paging, minId
cursors, and 429 backoff (Retry-After / X-RateLimit-Reset) are all exercised
with scripted transports in unit tests.
