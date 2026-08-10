# Trial 2: Repair-cafe tool lending kiosk

## Persona

You are the volunteer developer for a neighborhood repair cafe. You have strong Python and SQL experience and are an intermediate Rust user. You maintain the kiosk alone, have little time for operations, and need future volunteers to understand the data without specialized infrastructure knowledge.

## Current system

One laptop at the cafe desk tracks 8,000 tools and spare parts, 1,200 borrowers, and about 100,000 historical checkout, return, repair, and retirement events. Volunteers currently edit several CSV files and run a Python script that regenerates a "current inventory" CSV. A crash or duplicate row can leave the files disagreeing, and reconstructing who had an item on a past date is manual.

The replacement is a local, single-user kiosk application. Exactly one process writes at a time; it does not need network sync, replication, or a server database. The old CSV files must be imported once and retained as read-only migration evidence.

## Pain point

Volunteers need a dependable answer to simple questions: Is this item lendable now? Who has it? When is it due? Why is it unavailable? What happened to it last month? They also need forgiving lookup by asset tag, serial number, tool name, and nickname. The current snapshot script cannot update state and history together or explain inconsistencies clearly.

## Constraints

- Run fully offline on a four-year-old laptop with 256 MiB available to the kiosk process.
- Use a single local data directory that can be backed up by copying it while the kiosk is closed; no database server or cloud account.
- Preserve exact event order and original source-row references from the CSV import.
- A checkout, return, repair-status change, or retirement must either update both current state and history or update neither after a process interruption.
- Replaying accepted events must deterministically reconstruct the same current inventory.
- Reject an impossible transition, duplicate event ID, unknown borrower, or unknown item without changing visible state.
- Search may be approximate, but opening a result must resolve to one exact item and its authoritative history.
- Keep the application and dependency set small enough for one volunteer maintainer.

## Baseline approach

Replace the CSV files with a conventional embedded SQL database. Use transactions for an append-only event table plus current-state tables, ordinary indexes for tags and serials, and built-in text search or normalized substring matching for names and nicknames. The developer estimates this as a two-day implementation using familiar tools.

## Acceptance criteria

1. Import a representative fixture with 8,000 items, 1,200 borrowers, and 100,000 events; report every malformed or contradictory source row without silently inventing state.
2. For a fixed golden set, answer current holder, due date, lendability, and historical status-at-time queries exactly.
3. Pass a table-driven state-transition suite covering checkout, return, repair, retirement, duplicate IDs, and invalid references; every rejected command leaves state and history unchanged.
4. Across interruption tests at each modeled write boundary, reopening shows either the complete old state or the complete new state, never a split state/history update.
5. Rebuild current inventory from accepted events and match the stored current view byte-for-byte across three runs.
6. Find a known item in the first five results for at least 45 of 50 typo, abbreviation, tag, serial, name, and nickname queries; every result opens the exact canonical item.
7. On the declared laptop, open the 100,000-event data set in under two seconds and keep warm single-item lookup p95 under 50 ms across 1,000 queries.
8. Compare code size, dependency burden, backup and recovery steps, and failure behavior with the embedded-SQL baseline before recommending adoption.

## Non-goals

- Multi-laptop synchronization, concurrent writers, remote access, or cloud backup.
- Payments, fines, identity verification, or legal record retention.
- Automated purchasing, demand forecasting, or repair recommendations.
- Image recognition or searching photographs of tools.
- Replacing the retained original CSV files as migration evidence.
- Scaling beyond one repair cafe in this trial.
