# Representative fixture

The executable deterministically generates the representative fixture so that the repository does not carry a large generated data file:

- 8,000 uniquely identified items with asset tag, serial, name, nickname, and source-row reference;
- 1,200 borrowers with source-row references;
- 100,000 ordered checkout/return events with immutable IDs and source-row references;
- five rejected rows: duplicate event ID, unknown item, unknown borrower, impossible return, and a malformed row missing its event ID.

Run `cargo run --release -- benchmark` to generate, validate, import, query, replay, and remove the fixture.
