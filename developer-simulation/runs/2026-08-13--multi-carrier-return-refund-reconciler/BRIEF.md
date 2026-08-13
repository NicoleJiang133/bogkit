# Multi-Carrier Return Refund Reconciler

## Developer persona

You are a commerce-platform backend developer with seven years of production
TypeScript and PostgreSQL experience and four months of Rust experience. You
own the returns service but not the warehouse, carrier, or payment systems.

## Existing system and baseline

PostgreSQL is authoritative for orders, captured payment, line quantities,
tax, discounts, prior refunds, and operator decisions. Three carriers deliver
status webhooks, two warehouses publish scan batches, and the payment provider
publishes refund results. A nightly Python job joins exported CSV files with
SQL queries and emits an operator review sheet. Operators resolve ambiguous
rows manually before a separate service issues refunds.

The baseline is slow and difficult to audit, but it has one essential safety
property: it never initiates payment-provider calls automatically.

## Concrete pain

Returns increasingly arrive in split parcels. A customer may put an item in a
different parcel than the label predicted, a warehouse may correct a mistaken
SKU scan, and carrier or payment events may be duplicated, delayed, or arrive
out of order. The nightly job can show the same unit twice or fail to explain
why a refund quantity changed. The team needs a deterministic proposed refund
plan with exact provenance and explicit review cases.

## Workload and data shape

Use an immutable daily snapshot containing:

- 25,000 return authorizations, 40,000 parcels, and 70,000 authorized lines;
- 250,000 carrier, warehouse, correction, and payment-result events;
- integer quantities and integer cents only;
- line-level paid subtotal, tax, discount, and prior-refund amounts;
- stable event IDs, source-system IDs, source timestamps, and ingestion IDs;
- at most four parcels and twelve line items per return.

The representative fixture must include ordinary returns plus at least 500
seeded adversarial returns. No customer names, addresses, free text, payment
tokens, or real commercial data may appear in fixtures or diagnostics.

## Operational constraints

- PostgreSQL remains authoritative; the prototype reads snapshots only.
- The output is an advisory plan and must never call a carrier, warehouse, or
  payment API.
- Every cent and physical unit must be accounted for exactly.
- A proposed refund may never exceed captured payment minus successful prior
  refunds, either per line or per return.
- The same immutable inputs must produce byte-identical output regardless of
  input file order or event order.
- A complete 25,000-return run should finish within 60 seconds and 512 MiB on
  the available machine; report the actual host and measurement limits.
- Publication is complete-report-only: failure must not leave a new partial or
  stale plan at the requested output path.

## Measurable acceptance criteria

1. A separately implemented reference calculation and the candidate agree on
   every proposed line quantity, cent amount, disposition, and provenance ID
   for the disclosed fixtures.
2. Ten deterministic shuffles of the same snapshot produce byte-identical
   reports.
3. Exact retries are idempotent. The same source event ID with different
   payload bytes is quarantined for review and changes no proposed refund.
4. A later warehouse correction supersedes only the identified earlier scan;
   corrections cannot silently create negative or duplicate unit counts.
5. Split parcels and substituted-item scans conserve authorized quantities.
   Unmatched, excess, or unauthorized units are review cases, not refundable
   units.
6. Successful, failed, pending, and duplicated payment results are handled
   distinctly. An inconclusive result never proposes a second refund.
7. Injected failures before and during report publication preserve either the
   prior complete report or no report, never a partial new report.
8. The representative run records elapsed time and available peak-memory
   evidence without generalizing beyond the measured host.

## Adversarial and fault cases

- duplicate events with identical and conflicting payloads;
- warehouse corrections received before the event they correct;
- carrier delivery after a warehouse scan and carrier events with regressing
  source timestamps;
- one authorized unit scanned into two parcels;
- substituted, unknown, and over-quantity SKUs;
- discounts and tax that require deterministic remainder-cent allocation;
- prior refund success followed by a delayed pending or failure event;
- process exit before output creation, after temporary output is complete, and
  immediately before final publication;
- output-path aliasing with any input where the host supports detection.

## Compact prototype boundary

Build a local command-line program over synthetic JSON or CSV snapshots. It
should validate inputs, derive one deterministic return-level plan, write a
canonical machine-readable report, verify that report independently, and
include a runnable disclosed fixture plus a generated representative workload.
Keep payment execution, HTTP services, database schema changes, authentication,
operator UI, and production deployment outside the prototype.

## Non-goals

- replacing PostgreSQL or the existing refund executor;
- predicting fraud, return intent, or item condition;
- fuzzy matching customer text or images;
- choosing business policy for unauthorized substitutions;
- implementing carrier APIs, payment APIs, or a distributed workflow engine;
- claiming production readiness from synthetic data.

## Dependency decision threshold

Retain a new dependency only if the runnable evidence shows a material
correctness, auditability, or operational advantage over the direct
snapshot/reference approach without weakening any authority, idempotency,
privacy, or complete-publication constraint. Reject it when it merely stores a
second copy, moves the reconciliation rules into a harder abstraction, adds an
unneeded subsystem, or cannot share the authoritative transaction boundary.
