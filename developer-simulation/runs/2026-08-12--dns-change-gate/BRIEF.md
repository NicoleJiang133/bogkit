# Trial 1: Authoritative DNS change-admission gate

## Developer role and Rust experience

You are a hosting-platform reliability engineer who maintains authoritative DNS for several internal product teams. You have seven years of production Go and shell experience. Your Rust experience is exact: four months, one completed command-line utility of about 1,800 lines, and no production Rust service ownership.

## Existing system and baseline

The existing system stores BIND-style master files in two directory snapshots: the currently served snapshot and a proposed snapshot produced by an internal control panel. A shell release job runs `named-checkzone` once per changed zone, then copies approved files into the existing deployment pipeline. That checker and the current deployment pipeline remain in place and authoritative.

The release job catches syntax errors but does not explain the semantic effect of a change across zone cuts. Reviewers currently inspect diffs by hand. They have missed accidental removal of glue, illegal alias coexistence, large TTL reductions, and SOA serial changes that do not advance under 32-bit serial arithmetic.

## Concrete pain

A valid-looking change can make a delegated child unreachable or leave caches serving an unintended answer for hours. Manual review is slow for generated zones and inconsistent around escaped names, inherited owners and TTLs, includes, and records reordered by the control panel. The team needs an offline admission report that says exactly what changed and blocks only on declared, testable safety rules. It must not deploy anything.

## Workload and data shape

The representative corpus contains 10,000 zones, 2,000,000 resource records, and 30,000 changed records between the old and proposed snapshots. Inputs are ordinary files containing a declared subset of DNS master-file syntax:

- `$ORIGIN`, `$TTL`, relative and absolute owner names, omitted owners, escaped label bytes, comments, parentheses, and decimal TTL units;
- SOA, NS, A, AAAA, CNAME, MX, TXT, SRV, CAA, and DS records;
- `$INCLUDE` with an optional origin and `$GENERATE` with an inclusive numeric range and one substitution token.

Record identity is the canonical tuple `(zone, absolute owner, type, canonical rdata)`. TTL is compared separately. TXT chunk boundaries remain significant. Input is bytes; non-ASCII label bytes are accepted only through master-file escapes, not by guessing a text encoding.

The fixture generator supplies 9,950 unchanged zones, 45 ordinary changed zones, and five adversarial zones. The reference corpus includes legal reorder-only edits, delegation additions and removals, in-bailiwick and out-of-bailiwick name servers, glue changes, alias conflicts, serial wraparound, and duplicate records.

## Operational constraints

- Run offline on a declared four-core Linux machine with 256 MiB of measured peak resident memory and no network access.
- Finish the representative corpus in 20 seconds after filesystem caches are warmed once; report both warm-up and three measured runs.
- Never read outside either snapshot root. Resolve includes relative to the including file, reject absolute paths and any normalized path that escapes its root, and detect include cycles.
- Expand at most 2,500,000 records total, 250,000 records per zone, 32 include levels, and 100,000 records from any one `$GENERATE` directive. Exceeding a limit is a blocking diagnostic, not truncation.
- Produce byte-identical UTF-8 JSON for the same logical inputs even if directory enumeration order or record order changes. Sort zones, owners, types, records, and diagnostics by rules stated in the prototype README.
- Write the report to a temporary sibling and rename it only after every selected zone has been parsed and checked. On any blocking input error, exit nonzero and publish no replacement report.

## Fault and adversarial cases

The test corpus must include all of the following:

- an include cycle, a `../` escape, an absolute include, a missing include, and a symlink inside the root that resolves outside it;
- a `$GENERATE` range above the declared cap and nested includes whose total expansion crosses the per-zone cap;
- an unterminated parenthesized record, an overlong escaped label, an invalid IPv4 address, an unknown mandatory field, and an SOA integer outside its allowed width;
- a CNAME owner that also has MX data, an apex missing exactly one required SOA or NS record, and duplicate canonical records written with different whitespace;
- removal of the only in-bailiwick glue address for a delegated name server, removal of glue that is still covered by a second address family, and an out-of-bailiwick name server that correctly requires no parent glue;
- SOA serial advance, equality, regression, and RFC 1982 half-range ambiguity, including `4294967295` to `0`;
- simulated process termination after temporary-report creation and after the final flush but before rename. Rerunning must yield either the previous complete report or the new complete report, never a partial JSON document.

## Measurable acceptance criteria

Each criterion is checked by a standalone harness command and fixture directory, without inspecting implementation internals:

1. **Parser agreement:** For every accepted fixture in the declared syntax subset, canonical record tuples match a checked-in oracle manifest exactly. All malformed fixtures exit nonzero and name the file, byte offset, and stable diagnostic code.
2. **Order invariance:** Twenty seeded permutations of directory entries and record order produce the same SHA-256 digest of the final report.
3. **Change accuracy:** Against an oracle containing 1,000 labeled changes, the report has zero false additions, removals, or TTL changes. Reorder-only and exact duplicate inputs produce no semantic change.
4. **Delegation rules:** All 60 labeled delegation fixtures receive the expected `allow`, `review`, or `block` verdict and the report cites the exact owner names and record types involved. The harness includes both valid and invalid glue cases.
5. **SOA arithmetic:** All 40 boundary fixtures, including wrap and half-range ambiguity, match the checked-in verdict table. Ambiguity must be `review`; equality or regression must be `block`.
6. **Containment and exhaustion safety:** Every include-escape, cycle, malformed-record, and expansion-bomb fixture terminates within two seconds, publishes no replacement report, and leaves a sentinel file outside the roots unread and unchanged. The harness checks the sentinel access time where supported and uses an unreadable sentinel as a second check.
7. **Crash publication:** At every injected publication fault point, the output path contains either the complete old digest or the complete new digest. A subsequent fault-free run produces the new digest.
8. **Resource target:** Three measured warm-cache runs over the representative corpus each finish within 20 seconds and stay at or below 256 MiB peak resident memory; the harness records wall time, peak memory, input digest, and machine description.
9. **Baseline coexistence:** The prototype runs `named-checkzone` results supplied as fixtures and never converts a baseline failure into an allow verdict. A disagreement is surfaced as `block` with both diagnostic codes.

## Explicit non-goals

- No live DNS queries, recursive resolution, dynamic updates, DNSSEC signing, key management, or deployment.
- No attempt to parse every vendor extension or every resource-record type. Unsupported syntax blocks with a stable diagnostic.
- No automated repair, serial bump, glue synthesis, or file rewrite.
- No service, database, web interface, repository integration, or approval workflow.
- No claim that the prototype replaces the existing syntax checker or proves Internet-wide reachability.

## Smallest self-contained prototype boundary

Build one offline CLI, one fixture generator, and one black-box verification harness. The CLI accepts `--old-root`, `--new-root`, `--policy`, and `--output`; it expands only the declared syntax subset, canonicalizes both snapshots, compares selected zones, applies the stated safety rules, and atomically emits one JSON report. The policy file supplies only numerical caps, permitted TTL-decrease percentage, and the set of zones to compare.

Do not connect it to the release job. Evaluate any new library or runtime dependency against the parser agreement, deterministic-output, containment, crash-publication, memory, and timing gates above. Reject the dependency and retain the shell-plus-existing-checker baseline if it cannot meet every safety gate, requires network or resident services, weakens unsupported-input handling, or adds more operational work than the advisory report removes. A documented rejection is a valid trial result.
