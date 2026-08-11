# Trial 1 brief: Cold-chain excursion state repair

## Persona and Rust experience

You are a platform engineer for a regional food distributor. You have six years of production Python experience and about one year of Rust experience. You are comfortable with iterators, enums, serialization, and ordinary error handling, but have not built a storage engine or an event-processing framework.

## Current system and baseline implementation

Four hundred freezer controllers send one temperature observation every 30 seconds. Gateways retain observations during outages and upload them later. A Python service appends every accepted line to daily compressed NDJSON files, then updates an SQLite table containing each freezer's current status. The archive files are the authority; the status database and incident summaries may be rebuilt.

The reducer sorts each upload batch but not the full history. It treats a repeated observation as new, and operators can submit a correction that replaces a bad reading only by rerunning a day-specific script. Current logic opens an excursion after five continuous minutes outside the configured temperature band and closes it after ten continuous minutes back inside the band. Configuration changes carry an effective timestamp because limits sometimes change during maintenance.

## Concrete pain, workload, and data shape

The service receives about 1.15 million observations per day. A record contains `observation_id`, `freezer_id`, `observed_at`, `received_at`, temperature in integer tenths of a degree, gateway sequence number, and an optional `replaces_observation_id`. A separate record changes a freezer's allowed minimum or maximum temperature from a stated effective time.

Gateway reconnects create duplicate uploads and batches up to six hours late. Corrections may arrive up to seven days later. The current service occasionally creates duplicate incidents, closes an incident too early after late data arrives, or shows a current status that cannot be reconciled with the archive. Operators need an exact list of observations and configuration versions that caused every open, close, or repair decision.

## Operational constraints

- Run without network access on one Linux facility server with two CPU cores, 512 MiB of memory, and ordinary SSD storage.
- Keep the compressed NDJSON archive authoritative and unchanged. Any new local state must be disposable and rebuildable from it.
- Preserve the last complete readable derived state if the process exits during an ingest or repair.
- Reject malformed timestamps, unknown correction targets, correction cycles, and configuration intervals with no valid limit; never publish a partial batch.
- Diagnostics may include freezer IDs and observation IDs but must not include facility names or free-form operator notes.
- Do not assume arrivals are in event-time order. Equivalent valid inputs must produce byte-for-byte identical exported incident summaries regardless of batch boundaries.

## Measurable acceptance criteria

1. Provide a runnable fixture generator and a small independent reference reducer covering normal readings, threshold boundaries, duplicate IDs, six-hour lateness, corrections, and backdated configuration changes.
2. For at least 100 generated seeds, the candidate's final current-state and incident export exactly matches the reference reducer after chronological input, shuffled batches, and randomly repeated batches.
3. Ingesting an already accepted observation or correction is idempotent: state, incident history, and exported bytes do not change.
4. A late record, correction, or backdated configuration change repairs every affected incident for that freezer and does not alter any unaffected freezer's exported result.
5. Injected exits at the documented commit boundaries leave either the complete prior state or the complete next state; a restart converges to the reference result without manual cleanup.
6. Rebuild 1,000,000 observations for 400 freezers in under 60 seconds while the measured peak resident memory remains below 384 MiB on the declared test machine. Report the machine and measurement command.
7. Every incident transition exports a deterministic reason and the exact contributing observation and configuration identifiers.

## Explicit non-goals

- Do not implement controller firmware, gateway transport, alert delivery, dashboards, authentication, replication, or archive compression.
- Do not predict failures or invent a probabilistic anomaly detector.
- Do not modify the authoritative archive format.
- Do not claim production suitability from a toy happy-path run. A justified no-fit conclusion is valid if a required property cannot be demonstrated.

## Smallest self-contained prototype boundary

Build one command-line program that consumes observation and configuration NDJSON, maintains disposable local derived state, and exports canonical JSON incident summaries. Include a fixture generator, the independent reference reducer, restart/fault tests at the storage boundary, and the one-million-record benchmark. Limit the prototype to the excursion rule described above and a seven-day correction horizon. Assess the repository as found; keep the experiment separate from its existing implementation and examples.
