# Dependencies, generated paths, and cleanup

## Dependencies

- Rust toolchain observed: `rustc 1.95.0`, `cargo 1.95.0`.
- Direct crates: `serde` with derive support and `serde_json`.
- Exact resolved versions are pinned in the archive's single `developer-simulation/Cargo.lock` and were built with `--offline --locked`.
- No BogKit crate, network, database, firewall, private log, production policy, or external policy engine is used.
- All policies, samples, generators, and seeds are synthetic.

## Declared measurement host

- macOS 26.5.2, build 25F84.
- arm64.
- `/usr/bin/time -l` peak resident set size is reported in bytes on this host.

## Deliverable cleanliness

- Source deliverable: `simulation-output/firewall-policy-impact`.
- No `target` directory or generated 50,000-rule JSON is stored in the deliverable.
- The root workspace manifests, root lockfile, BogKit crates, public examples, Git state, GitHub state, and automation state were not modified.

## Generated temporary paths used during verification

- `/private/tmp/bogkit-sim-trial2-target` — debug/release build output.
- `/private/tmp/bogkit-trial2-baseline-report.json` and `...-2.json` — deterministic release reports.
- `/private/tmp/bogkit-trial2-benchmark-strong-0809` — final 50,000-rule benchmark inputs/report.
- `/private/tmp/bogkit-trial2-benchmark-0809` — earlier, easier shadow-heavy benchmark discarded in favor of the stronger mixed reachability fixture.
- `/private/tmp/bogkit-trial2-baseline-report.json` and benchmark files contain synthetic data only.
- `/private/tmp/bogkit-trial2-invalid-report.json` was confirmed absent after the invalid-input run.

These exact temporary targets may be deleted after review:

```console
$ rm -rf /private/tmp/bogkit-sim-trial2-target
$ rm -rf /private/tmp/bogkit-trial2-benchmark-strong-0809
$ rm -rf /private/tmp/bogkit-trial2-benchmark-0809
$ rm -f /private/tmp/bogkit-trial2-baseline-report.json
$ rm -f /private/tmp/bogkit-trial2-baseline-report-2.json
```

They are intentionally left available at completion so the coordinator can inspect the just-measured synthetic artifacts. Regeneration commands are in `README.md` and `evidence/COMMANDS.md`.

Reviewer fix round 1 used only paths inside this deliverable: `.reviewer-target`, `.reviewer-benchmark`, and `.reviewer-report-{a,b}.json`. They were removed after their results were recorded. The adversarial invalid run created no `.reviewer-reserved-report.json`.

Reviewer fix round 2 likewise used only in-deliverable hidden paths: `.reviewer2-target`, `.reviewer2-benchmark`, `.reviewer2-report*.json`, and short-lived `.reused-output-test-*`/`.output-alias-test-*` test directories. All were removed or self-cleaned after successful verification. A final hidden-directory/build-artifact scan is recorded in `evidence/COMMANDS.md`.

Reviewer fix round 3 used `.reviewer3-target`, `.reviewer3-benchmark`, `.reviewer3-report*.json`, and self-cleaning `.publication-test-*` directories, all inside the deliverable boundary. They were removed after the recorded run, followed by the same hidden target/temp/symlink scan.
