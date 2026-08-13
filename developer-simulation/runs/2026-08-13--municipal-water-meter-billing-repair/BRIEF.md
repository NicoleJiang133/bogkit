# Municipal Water-Meter Billing Repair

## Developer persona

You are a municipal-utility data engineer with nine years of production SQL
and Python experience and one year of Rust experience. You maintain billing
imports and adjustment tooling, while a separate vendor system owns customer
accounts and invoice issuance.

## Existing system and baseline

The authoritative billing database stores cumulative meter readings, meter
installation history, billing cycles, issued bills, and approved adjustments.
Field devices send daily batches through an older import service. A collection
of SQL views and Python scripts reconstructs consumption when a meter rolls
over, is replaced, reports an estimate, or later sends a correction. Staff
compare spreadsheet extracts before importing approved adjustment rows.

The baseline remains the production authority. It is cumbersome but its SQL
views are understood by auditors and it never edits an issued bill directly.

## Concrete pain

Late corrections and meter replacements can change previously derived usage.
The current scripts sometimes recompute too broad a date range, and their
ordering assumptions are implicit. Staff need an exact, deterministic repair
plan that names the readings and installation records behind every changed
billing interval and clearly refuses ambiguous histories.

## Workload and data shape

Use immutable exports containing:

- 100,000 service points and 120,000 physical meters;
- 2,000,000 cumulative readings spanning 24 monthly billing cycles;
- six- or eight-digit integer registers measured in whole litres;
- actual, estimated, and corrected readings with stable source IDs;
- explicit meter-install and meter-remove instants plus register width;
- 200,000 previously billed intervals and any prior adjustment records.

The disclosed fixtures must cover hand-checkable ordinary and adversarial
histories. A deterministic generator should build the representative scale
without using resident, customer, address, or real meter data.

## Operational constraints

- The vendor billing database remains authoritative and is not modified.
- All volumes and monetary-free adjustments use exact integers; floating point
  is prohibited in the repair calculation.
- Input order and batch boundaries must not affect the result.
- Every changed interval needs exact source-reading and meter-installation
  provenance, plus a stable reason code.
- Ambiguous histories fail closed for that service point and do not suppress
  valid plans for unrelated service points.
- A representative two-million-reading run should finish within 45 seconds
  and 384 MiB on the available machine; state the measured host and limits.
- Report publication must be deterministic and complete, with a prior report
  preserved on validation or computation failure.

## Measurable acceptance criteria

1. The candidate agrees exactly with a separately written chronological
   reference reducer on all disclosed cases and 100 deterministic generated
   seeds.
2. Ten input permutations and at least three different batch sizes produce
   byte-identical canonical repair plans.
3. Exact duplicate readings are idempotent. Conflicting reuse of a source ID
   is rejected before changing any derived interval.
4. Six- and eight-digit rollovers are distinguished from ordinary regression
   by explicit meter metadata. Half-range or otherwise ambiguous regressions
   become review cases rather than invented consumption.
5. Meter replacement closes the old register and opens the new register only
   at the declared installation boundary. Overlap, gaps without policy, and
   readings attached to the wrong meter fail closed.
6. A corrected actual supersedes exactly its named reading. An actual reading
   that supersedes an estimate repairs only intervals causally depending on
   that estimate.
7. Already issued intervals are never rewritten. The output contains explicit
   old usage, recomputed usage, signed integer adjustment, provenance, and
   reason; unchanged intervals are omitted.
8. A failed input or injected publication failure leaves the previous complete
   report byte-for-byte unchanged.
9. The representative run reports elapsed time and available peak-memory
   evidence without treating the local host as production qualification.

## Adversarial and fault cases

- corrections arriving before or after the reading they supersede;
- duplicate and conflicting source IDs;
- a register value at the maximum, rollover to zero, ordinary regression, and
  ambiguous half-range regression;
- meter replacement at the exact boundary timestamp, overlapping installs,
  missing removal, and a reading for an inactive meter;
- estimates later replaced by actuals across one or multiple billing cycles;
- out-of-order readings with equal timestamps and stable tie-breaking;
- malformed register width or values outside the declared range;
- process exit before temporary publication and immediately before final
  publication;
- output-path identity with an input where detectable.

## Compact prototype boundary

Build an offline command-line program that reads synthetic newline-delimited or
JSON exports, validates each service-point history, emits one canonical repair
plan, and verifies it against an independently derived reference. Include
hand-checkable fixtures, generated seeded trials, a representative-scale
demonstration, and exact reproduction commands. Keep database connections,
billing rates, money calculation, invoice issuance, customer UI, device
protocols, and production migrations outside the prototype.

## Non-goals

- replacing the billing database, import service, or adjustment approval flow;
- estimating missing consumption with a new business policy;
- changing bills or issuing credits;
- forecasting demand, detecting leaks, or performing anomaly scoring;
- accepting ambiguous meter histories automatically;
- claiming regulatory or production qualification.

## Dependency decision threshold

Retain a new dependency only if it demonstrably reduces the repair logic or
improves exact replay, provenance, bounded recomputation, or safe publication
over the direct chronological reference while staying within the authority and
resource limits. Reject it if the application still owns all temporal rules,
if it creates a second durable authority, if it adds unrelated capabilities,
or if its operational cost is not justified by measured improvement.
