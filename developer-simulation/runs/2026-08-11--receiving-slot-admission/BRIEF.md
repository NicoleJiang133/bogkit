# Trial 2 brief: Receiving-dock slot admission

## Persona and Rust experience

You are a backend engineer for a warehouse-management vendor. You have eight years of production TypeScript and PostgreSQL experience and three months of Rust experience. You understand transactions and row locks well, but Rust ownership and generic-heavy APIs are still unfamiliar.

## Current system and baseline implementation

The existing service lets carriers hold and confirm 15-minute receiving slots at 50 warehouses. PostgreSQL is authoritative for warehouses, doors, holds, confirmed bookings, and an append-only decision audit. Six stateless Node.js service replicas use serializable transactions and advisory locks. Redis currently expires tentative holds, while a database job releases capacity later.

This split causes trouble: a Redis expiry can race with confirmation, retries after timeouts can create a second hold, and the cleanup job can temporarily leave capacity unavailable. The team wants to evaluate whether this repository can provide a smaller, clearer admission core, but it must not displace PostgreSQL or weaken the existing transaction boundary.

## Concrete pain, workload, and data shape

Each warehouse has 8 to 40 doors. A door has a vehicle-height class, optional refrigeration, and a maximum pallet intake per 15-minute interval. A booking request contains `tenant_id`, `request_id`, warehouse, arrival interval, duration from one to eight intervals, pallet count, height class, refrigeration requirement, carrier priority, and a hold lifetime no longer than 15 minutes. Confirm, cancel, and reschedule commands reference the original request ID.

The service handles roughly 120,000 bookings per day and peaks at 500 commands per second. A request may use any one compatible door, but all intervals of a booking must use the same door. Confirmed bookings outrank holds; holds otherwise use first-accepted order. Capacity cannot be oversubscribed. API clients retry aggressively after timeouts, so the same request ID may arrive concurrently at different replicas. Operations needs stable machine-readable reasons for rejection: no compatible door, pallet capacity, time conflict, expired hold, stale reschedule version, or request-ID payload mismatch.

## Operational constraints

- PostgreSQL remains the only production source of truth, and the accepted command plus its decision-audit row must commit in the same database transaction.
- Six replicas may process commands concurrently. No correctness argument may depend on process-local memory, one permanently elected application leader, or clock agreement between replicas.
- Expiry decisions use a database-supplied timestamp captured by the command transaction. Tests use an injected logical timestamp; wall-clock sleeps are not acceptable evidence.
- A retry with the same tenant and request ID must return the original result. Reusing that ID with a different payload must fail visibly.
- After an ambiguous client timeout or process exit, retrying must not consume capacity twice, revive an expired hold, or lose the audit decision.
- The existing HTTP API, PostgreSQL schema ownership, migrations, and operations tooling must remain recognizable. A local side database is not acceptable production authority.

## Measurable acceptance criteria

1. Supply a deterministic command fixture and an independent simple reference model for one warehouse with 24 doors and at least 10,000 mixed hold, confirm, cancel, reschedule, expiry, and retry commands.
2. For 100 generated seeds, every accepted or rejected command matches the reference model's resulting booking state and reason code when executed sequentially with logical timestamps.
3. Run a concurrency test with at least four independent workers issuing colliding commands against the same production-shaped authority. Across 30 runs, no door interval exceeds its pallet capacity, no confirmed booking overlaps another confirmed booking on the same door, and each request ID has exactly one decision outcome.
4. Demonstrate retries after injected exits immediately before and after the transaction commit. Each retry returns the durable original outcome, and each accepted state change has exactly one corresponding audit record.
5. Expiring a hold and confirming it at the same logical timestamp has one documented, deterministic rule that is identical across workers and test runs.
6. On the declared test machine, replay 100,000 representative commands at an observed throughput of at least 250 commands per second with p95 decision latency below 40 ms. Report database version, schema size, machine, and measurement method.
7. Rejection explanations are stable enums with relevant door or interval identifiers; they must not depend on unordered iteration or include other tenants' request data.

## Explicit non-goals

- Do not build the HTTP layer, carrier portal, authentication, notifications, billing, route optimization, or a general constraint solver.
- Do not redesign the PostgreSQL schema beyond the minimal tables or indexes needed for the prototype.
- Do not use Redis, wall-clock timers, or a new database to hide a missing transactional guarantee.
- Do not assume this repository is suitable. A documented no-fit result with runnable evidence is a successful trial.

## Smallest self-contained prototype boundary

Implement only the admission core and a command-line test harness for one 24-door warehouse using a disposable PostgreSQL database. Cover holds, confirmation, cancellation, rescheduling, expiry, idempotent request IDs, decision auditing, and the listed door constraints. Use deterministic fixtures and logical timestamps, and run both sequential reference checks and real multi-worker collision tests. Keep the experiment separate from the repository's existing implementation and examples; if its available abstractions cannot share the required PostgreSQL transaction, stop at the smallest reproducer and record the decision boundary rather than building an unrelated replacement.
