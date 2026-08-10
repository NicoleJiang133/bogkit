# Trial 1: Support-case duplicate finder

## Persona

You are a customer-support platform developer with six years of production TypeScript experience and about six months of Rust experience. You maintain internal tools for a 35-person support team and favor small, observable services over introducing a new platform.

## Current system

The support application stores open and closed cases in PostgreSQL. A worker exports an approved, privacy-scrubbed projection of closed cases containing case ID, language, product area, subject, cleaned body, resolution summary, and last-modified time. Agents currently search that projection through PostgreSQL full-text search before writing a new investigation.

The export contains 75,000 English and Spanish cases and changes by roughly 2,000 inserts, edits, or deletions per day. PostgreSQL remains the authority for case visibility and permissions. The new feature may build a replaceable local index from the approved projection, but it must never become the case system of record.

## Pain point

Lexical search misses useful prior resolutions when the new report uses different wording, abbreviations, or the other supported language. Agents spend time rediscovering known fixes. The team wants an advisory "related resolved cases" panel, but only if it measurably improves retrieval and preserves exact, inspectable source references.

## Constraints

- Run as one ordinary service process on a 2-core, 2 GiB container; no GPU is available in production.
- After build and index refresh, query handling must not call an external model or search service.
- Store only the approved scrubbed projection; do not ingest attachments, raw customer messages, names, email addresses, or account identifiers.
- Apply daily insert, edit, and deletion batches. A failed refresh must leave the previously complete index queryable.
- Results are advisory and must include the exact case ID, product area, language, and a non-generated source excerpt so an agent can inspect the original record.
- Break equal scores deterministically by case ID. Deleted or newly restricted cases must disappear after the next successful refresh.
- Keep deploy-time dependencies and ongoing operations small enough for the existing team to own.

## Baseline approach

Keep PostgreSQL full-text search with a small synonym table and language-specific configurations. On a fixed 200-query evaluation set assembled from real, privacy-scrubbed duplicate investigations, this returns a known useful case in the first five results for 116 queries (58% recall@5). Its warm p95 latency is 34 ms.

## Acceptance criteria

1. On the same frozen 200-query set, return a known useful case in the first five results for at least 144 queries (72% recall@5), with separate English and Spanish results reported.
2. Answer the full evaluation set with warm p95 latency at or below 100 ms on the declared 2-core machine, with no network access during queries.
3. Build the 75,000-case index in at most 10 minutes, keep its on-disk size at or below 750 MiB, and apply a representative 2,000-record mixed update batch in at most 90 seconds.
4. Produce byte-for-byte identical ordered case IDs across three runs against the same index and queries.
5. Demonstrate that an interrupted refresh keeps the previous complete index usable, then that a successful retry includes edits and excludes all deleted cases.
6. Every displayed result contains only approved fields and an exact source excerpt; a fixture scan finds none of the prohibited personal fields.
7. Compare implementation size, dependencies, refresh operations, and measured retrieval quality against the lexical baseline before recommending adoption.

## Non-goals

- Generating answers or summaries for agents.
- Automatically merging, closing, routing, or prioritizing cases.
- Replacing PostgreSQL, authentication, authorization, or the support application.
- Training or fine-tuning a model.
- Supporting arbitrary languages beyond English and Spanish in this trial.
- Building a distributed search cluster or a public customer-facing search API.
