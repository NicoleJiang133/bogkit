use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_ID: AtomicU64 = AtomicU64::new(0);

struct Case {
    root: PathBuf,
    old: PathBuf,
    new: PathBuf,
    policy: PathBuf,
    output: PathBuf,
}

impl Case {
    fn new(name: &str) -> Self {
        let id = NEXT_ID.fetch_add(1, Ordering::Relaxed);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("target")
            .join("black-box")
            .join(format!("{name}-{}-{id}", std::process::id()));
        let old = root.join("old");
        let new = root.join("new");
        fs::create_dir_all(&old).expect("create old root");
        fs::create_dir_all(&new).expect("create new root");
        Self {
            policy: root.join("policy.conf"),
            output: root.join("report.json"),
            root,
            old,
            new,
        }
    }

    fn zone(&self, snapshot: &Path, contents: &str) {
        assert!(
            snapshot.starts_with(&self.root),
            "fixture write escaped case root"
        );
        fs::write(snapshot.join("example.zone"), contents).expect("write zone");
    }

    fn policy(&self, extra: &str) {
        let base = concat!(
            "max_records_total=2500000\n",
            "max_records_per_zone=250000\n",
            "max_include_depth=32\n",
            "max_generate_records=100000\n",
            "ttl_decrease_percent=50\n",
            "zone=example.test.|example.zone|PASS|PASS\n",
        );
        fs::write(&self.policy, format!("{base}{extra}")).expect("write policy");
    }

    fn raw_policy(&self, text: &str) {
        fs::write(&self.policy, text).expect("write policy");
    }

    fn run(&self) -> Output {
        self.run_to(&self.output)
    }

    fn run_to(&self, output: &Path) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dns-change-gate"))
            .args([
                "--old-root",
                self.old.to_str().expect("old path"),
                "--new-root",
                self.new.to_str().expect("new path"),
                "--policy",
                self.policy.to_str().expect("policy path"),
                "--output",
                output.to_str().expect("output path"),
            ])
            .output()
            .expect("run gate")
    }

    fn run_with_fault(&self, fault: &str) -> Output {
        Command::new(env!("CARGO_BIN_EXE_dns-change-gate"))
            .args([
                "--old-root",
                self.old.to_str().expect("old path"),
                "--new-root",
                self.new.to_str().expect("new path"),
                "--policy",
                self.policy.to_str().expect("policy path"),
                "--output",
                self.output.to_str().expect("output path"),
            ])
            .env("DNS_GATE_FAULT", fault)
            .output()
            .expect("run gate with fault")
    }
}

impl Drop for Case {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.root);
    }
}

const OLD: &str = r#"$ORIGIN example.test.
$TTL 1h
@ IN SOA ns1 hostmaster ( 4294967295 1h 15m 1w 5m )
@ IN NS ns1
ns1 IN A 192.0.2.1
www 300 IN A 192.0.2.10
child IN NS ns.child
ns.child IN A 192.0.2.53
mail IN MX 10 mx
mx IN AAAA 2001:db8::1
text IN TXT "one" "two"
"#;

const NEW: &str = r#"$TTL 3600
$ORIGIN example.test.
text IN TXT "one" "two"
@ IN SOA ns1.example.test. hostmaster.example.test. ( 0 3600 900 604800 300 )
@ NS ns1
ns1 A 192.0.2.1
www 5m A 192.0.2.11
child NS ns.child
ns.child A 192.0.2.53
mail MX 10 mx
mx AAAA 2001:db8:0:0:0:0:0:1
"#;

#[test]
fn valid_change_report_is_canonical_and_deterministic() {
    let case = Case::new("canonical");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");

    let first = case.run();
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let report = fs::read_to_string(&case.output).expect("published report");
    assert!(report.ends_with('\n'));
    assert!(report.contains("\"overall_verdict\":\"allow\""));
    assert!(
        report.contains("\"owner\":\"www.example.test.\",\"type\":\"A\",\"rdata\":\"192.0.2.10\"")
    );
    assert!(
        report.contains("\"owner\":\"www.example.test.\",\"type\":\"A\",\"rdata\":\"192.0.2.11\"")
    );
    assert!(report.contains("\"code\":\"SOA_ADVANCE\""));
    assert!(
        !report.contains("TTL_DECREASE"),
        "300 to 300 is not a TTL change"
    );

    let first_bytes = fs::read(&case.output).expect("first report bytes");
    let second = case.run();
    assert!(second.status.success());
    assert_eq!(
        first_bytes,
        fs::read(&case.output).expect("second report bytes")
    );
}

#[test]
fn reorder_duplicates_omitted_owners_and_escaped_labels_have_no_change() {
    let case = Case::new("reorder");
    let old = concat!(
        "$ORIGIN example.test.\n$TTL 1h\n",
        "@ SOA ns hostmaster 7 1h 15m 1w 5m\n",
        "@ NS ns\nns A 192.0.2.1\n",
        "odd\\046label A 192.0.2.2\n",
        "  AAAA 2001:db8::2\n",
        "txt TXT \"a\" \"b\"\n",
    );
    let new = concat!(
        "$TTL 3600\n$ORIGIN example.test.\n",
        "txt TXT \"a\" \"b\"\n",
        "odd\\046label AAAA 2001:db8:0:0:0:0:0:2\n",
        "odd\\046label A 192.0.2.2\n",
        "odd\\046label A 192.0.2.2 ; exact duplicate\n",
        "@ NS ns.example.test.\n",
        "ns A 192.0.2.1\n",
        "@ SOA ns.example.test. hostmaster.example.test. 7 3600 900 604800 300\n",
    );
    case.zone(&case.old, old);
    case.zone(&case.new, new);
    case.policy("");
    let result = case.run();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&case.output).expect("report");
    assert!(report.contains("\"changes\":[]"));
    assert!(!report.contains("SOA_EQUAL"));
}

#[test]
fn semantic_blocks_and_reviews_are_reported_with_exact_evidence() {
    let cases = [
        (
            "alias",
            "@ SOA ns hostmaster 8 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\nbad CNAME target\nbad MX 10 mail\n",
            "CNAME_COEXISTENCE",
            "bad.example.test.",
        ),
        (
            "missing-apex-ns",
            "@ SOA ns hostmaster 8 1h 15m 1w 5m\nns A 192.0.2.1\n",
            "APEX_NS_MISSING",
            "example.test.",
        ),
        (
            "removed-glue",
            "@ SOA ns hostmaster 8 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\nchild NS ns.child\n",
            "REMOVED_LAST_GLUE",
            "child.example.test.",
        ),
    ];
    for (name, new_body, code, owner) in cases {
        let case = Case::new(name);
        case.zone(&case.old, OLD);
        case.zone(
            &case.new,
            &format!("$ORIGIN example.test.\n$TTL 1h\n{new_body}"),
        );
        case.policy("");
        let result = case.run();
        assert_eq!(
            result.status.code(),
            Some(2),
            "{name}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = fs::read_to_string(&case.output).expect("blocking semantic report");
        assert!(
            report.contains(&format!("\"code\":\"{code}\"")),
            "{name}: {report}"
        );
        assert!(report.contains(owner), "{name}: {report}");
    }

    let case = Case::new("half-range");
    case.zone(&case.old, &minimal_zone(1, "child NS ns.external.test.\n"));
    case.zone(
        &case.new,
        &minimal_zone(2_147_483_649, "child NS ns.external.test.\n"),
    );
    case.policy("");
    let result = case.run();
    assert!(
        result.status.success(),
        "review is published with zero exit: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&case.output).expect("review report");
    assert!(report.contains("\"overall_verdict\":\"review\""));
    assert!(report.contains("SOA_HALF_RANGE_AMBIGUOUS"));
    assert!(report.contains("OUT_OF_BAILIWICK_NO_GLUE"));
}

#[test]
fn soa_serial_boundaries_match_literal_oracle() {
    let oracle = [
        (10, 11, "SOA_ADVANCE", 0),
        (u32::MAX, 0, "SOA_ADVANCE", 0),
        (10, 10, "SOA_EQUAL", 2),
        (10, 9, "SOA_REGRESSION", 2),
        (0, 1_u32 << 31, "SOA_HALF_RANGE_AMBIGUOUS", 0),
    ];
    for (old_serial, new_serial, code, status) in oracle {
        let case = Case::new(code);
        case.zone(&case.old, &minimal_zone(old_serial, "www A 192.0.2.1\n"));
        case.zone(&case.new, &minimal_zone(new_serial, "www A 192.0.2.2\n"));
        case.policy("");
        let result = case.run();
        assert_eq!(
            result.status.code(),
            Some(status),
            "{code}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let report = fs::read_to_string(&case.output).expect("serial report");
        assert!(report.contains(&format!("\"code\":\"{code}\"")), "{report}");
    }
}

#[test]
fn parser_and_containment_failures_preserve_previous_report() {
    let malformed = [
        (
            "unterminated",
            "@ SOA ns hostmaster ( 2 1h 15m 1w 5m\n",
            "E_UNTERMINATED_PAREN",
        ),
        (
            "bad-ip",
            "@ SOA ns hostmaster 2 1h 15m 1w 5m\n@ NS ns\nns A 999.2.3.4\n",
            "E_RDATA",
        ),
        (
            "unknown-field",
            "@ SOA ns hostmaster 2 1h 15m 1w 5m\n@ NS ns\nns CHAOS A 192.0.2.1\n",
            "E_FIELD",
        ),
        (
            "soa-width",
            "@ SOA ns hostmaster 4294967296 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\n",
            "E_INTEGER_WIDTH",
        ),
    ];
    for (name, body, code) in malformed {
        let case = Case::new(name);
        case.zone(&case.old, OLD);
        case.zone(
            &case.new,
            &format!("$ORIGIN example.test.\n$TTL 1h\n{body}"),
        );
        case.policy("");
        fs::write(&case.output, b"OLD-COMPLETE\n").expect("sentinel report");
        let result = case.run();
        assert_eq!(result.status.code(), Some(1), "{name}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains(code), "{name}: {stderr}");
        assert!(stderr.contains("example.zone:"), "{name}: {stderr}");
        assert_eq!(
            fs::read(&case.output).expect("old report"),
            b"OLD-COMPLETE\n"
        );
    }

    let include_cases = [
        ("parent", "$INCLUDE ../sentinel\n", "E_INCLUDE_ESCAPE"),
        ("absolute", "$INCLUDE /etc/passwd\n", "E_INCLUDE_ABSOLUTE"),
        ("missing", "$INCLUDE absent.zone\n", "E_INCLUDE_MISSING"),
        ("cycle", "$INCLUDE example.zone\n", "E_INCLUDE_CYCLE"),
    ];
    for (name, directive, code) in include_cases {
        let case = Case::new(name);
        case.zone(&case.old, OLD);
        case.zone(&case.new, directive);
        case.policy("");
        fs::write(&case.output, b"OLD-COMPLETE\n").expect("sentinel report");
        let result = case.run();
        assert_eq!(result.status.code(), Some(1), "{name}");
        assert!(String::from_utf8_lossy(&result.stderr).contains(code));
        assert_eq!(
            fs::read(&case.output).expect("old report"),
            b"OLD-COMPLETE\n"
        );
    }
}

#[cfg(unix)]
#[test]
fn symlink_escape_is_rejected_before_target_contents_are_read() {
    use std::os::unix::fs::{PermissionsExt, symlink};

    let case = Case::new("symlink");
    case.zone(&case.old, OLD);
    let sentinel = case.root.join("outside-sentinel");
    fs::write(&sentinel, b"SECRET-MUST-NOT-BE-PARSED\n").expect("outside sentinel");
    let accessed_before = fs::metadata(&sentinel).and_then(|metadata| metadata.accessed());
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o000))
        .expect("make sentinel unreadable");
    symlink(&sentinel, case.new.join("linked.zone")).expect("symlink");
    case.zone(&case.new, "$INCLUDE linked.zone\n");
    case.policy("");
    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_INCLUDE_SYMLINK"));
    fs::set_permissions(&sentinel, fs::Permissions::from_mode(0o600))
        .expect("restore sentinel permission");
    if let (Ok(before), Ok(after)) = (
        accessed_before,
        fs::metadata(&sentinel).and_then(|metadata| metadata.accessed()),
    ) {
        assert_eq!(before, after, "sentinel contents must not be opened");
    }
    assert_eq!(
        fs::read(&sentinel).expect("sentinel unchanged"),
        b"SECRET-MUST-NOT-BE-PARSED\n"
    );
    assert!(!case.output.exists());
}

#[test]
fn generate_and_nested_include_limits_block_without_truncation() {
    let case = Case::new("generate-limit");
    case.zone(&case.old, OLD);
    case.zone(
        &case.new,
        "$ORIGIN example.test.\n$TTL 1h\n$GENERATE 1-4 host$ A 192.0.2.1\n",
    );
    case.raw_policy(concat!(
        "max_records_total=20\nmax_records_per_zone=10\nmax_include_depth=2\n",
        "max_generate_records=3\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|PASS|PASS\n",
    ));
    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_GENERATE_LIMIT"));
    assert!(!case.output.exists());

    let case = Case::new("zone-limit");
    case.zone(&case.old, OLD);
    case.zone(
        &case.new,
        "$ORIGIN example.test.\n$TTL 1h\n$INCLUDE one.inc\n",
    );
    fs::write(
        case.new.join("one.inc"),
        "$INCLUDE two.inc\na A 192.0.2.1\n",
    )
    .expect("include one");
    fs::write(case.new.join("two.inc"), "b A 192.0.2.2\nc A 192.0.2.3\n").expect("include two");
    case.raw_policy(concat!(
        "max_records_total=100\nmax_records_per_zone=2\nmax_include_depth=3\n",
        "max_generate_records=10\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|PASS|PASS\n",
    ));
    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_ZONE_LIMIT"));
    assert!(!case.output.exists());
}

#[test]
fn include_depth_limit_blocks_before_deeper_file_is_parsed() {
    let case = Case::new("include-depth");
    case.zone(&case.old, OLD);
    case.zone(
        &case.new,
        "$ORIGIN example.test.\n$TTL 1h\n$INCLUDE one.inc\n",
    );
    fs::write(case.new.join("one.inc"), "$INCLUDE two.inc\n").expect("first include");
    fs::write(case.new.join("two.inc"), "$INCLUDE three.inc\n").expect("second include");
    fs::write(case.new.join("three.inc"), "bomb A 999.999.999.999\n").expect("unreached file");
    case.raw_policy(concat!(
        "max_records_total=100\nmax_records_per_zone=100\nmax_include_depth=2\n",
        "max_generate_records=10\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|PASS|PASS\n",
    ));
    let started = std::time::Instant::now();
    let result = case.run();
    assert!(started.elapsed() < std::time::Duration::from_secs(2));
    assert_eq!(result.status.code(), Some(1));
    let stderr = String::from_utf8_lossy(&result.stderr);
    assert!(stderr.contains("E_INCLUDE_DEPTH"), "{stderr}");
    assert!(
        !stderr.contains("E_RDATA"),
        "deeper malformed content was parsed: {stderr}"
    );
    assert!(!case.output.exists());
}

#[test]
fn baseline_failure_can_never_become_allow() {
    let case = Case::new("baseline");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.raw_policy(concat!(
        "max_records_total=2500000\nmax_records_per_zone=250000\nmax_include_depth=32\n",
        "max_generate_records=100000\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|OLD_OK|NEW_NAMED_CHECKZONE_FAIL\n",
    ));
    let result = case.run();
    assert_eq!(result.status.code(), Some(2));
    let report = fs::read_to_string(&case.output).expect("baseline block report");
    assert!(report.contains("BASELINE_FAILURE"));
    assert!(report.contains("OLD_OK"));
    assert!(report.contains("NEW_NAMED_CHECKZONE_FAIL"));
}

#[test]
fn publication_faults_leave_old_complete_report_then_rerun_replaces_it() {
    let case = Case::new("publication");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");
    for fault in ["after-temp", "after-flush"] {
        fs::write(&case.output, b"{\"old\":true}\n").expect("old report");
        let result = case.run_with_fault(fault);
        assert!(matches!(result.status.code(), Some(86 | 87)));
        assert_eq!(
            fs::read(&case.output).expect("complete old report"),
            b"{\"old\":true}\n"
        );
    }
    let result = case.run();
    assert!(result.status.success());
    let report = fs::read_to_string(&case.output).expect("new report");
    assert!(report.starts_with("{\"schema_version\":1,"));
    assert!(report.ends_with("}\n"));
}

#[test]
fn preexisting_temp_collision_survives_repeated_publication_without_owned_residue() {
    let case = Case::new("temp-collision");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");
    let marker = case.root.join("collision-path");
    let first = Command::new("sh")
        .args([
            "-c",
            concat!(
                "collision=\"$OUTPUT_PARENT/.$OUTPUT_NAME.tmp-$$-00000000000000000000000000000001-0000000000000000-000\"\n",
                "printf '%s' \"$collision\" > \"$MARKER\"\n",
                "printf 'DO-NOT-TOUCH\\n' > \"$collision\"\n",
                "exec \"$BIN\" --old-root \"$OLD_ROOT\" --new-root \"$NEW_ROOT\" ",
                "--policy \"$POLICY\" --output \"$OUTPUT\"\n",
            ),
        ])
        .env("BIN", env!("CARGO_BIN_EXE_dns-change-gate"))
        .env("OLD_ROOT", &case.old)
        .env("NEW_ROOT", &case.new)
        .env("POLICY", &case.policy)
        .env("OUTPUT", &case.output)
        .env("OUTPUT_PARENT", &case.root)
        .env("OUTPUT_NAME", "report.json")
        .env("MARKER", &marker)
        .env("DNS_GATE_TEMP_NONCE", "1")
        .output()
        .expect("run with a pre-existing legacy temp entry");
    assert!(
        first.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&first.stderr)
    );
    let collision = PathBuf::from(fs::read_to_string(&marker).expect("collision marker"));
    assert_eq!(
        fs::read(&collision).expect("pre-existing entry remains"),
        b"DO-NOT-TOUCH\n"
    );

    let second = case.run();
    assert!(second.status.success());
    let temp_entries = publication_temp_entries(&case.root);
    assert_eq!(temp_entries, vec![collision]);
}

#[test]
fn normal_publication_failure_removes_only_its_owned_temp_file() {
    let case = Case::new("temp-cleanup");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");
    fs::create_dir(&case.output).expect("blocking output directory");
    let sentinel = case.output.join("keep");
    fs::write(&sentinel, b"KEEP\n").expect("output directory sentinel");

    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_OUTPUT"));
    assert_eq!(fs::read(&sentinel).expect("sentinel retained"), b"KEEP\n");
    assert!(publication_temp_entries(&case.root).is_empty());
}

#[test]
fn output_inside_input_snapshot_is_rejected_before_publication() {
    let case = Case::new("output-in-snapshot");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");
    let aliased_output = case.old.join("example.zone");
    let original = fs::read(&aliased_output).expect("original selected input");

    let result = case.run_to(&aliased_output);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_OUTPUT_ALIAS"));
    assert_eq!(
        fs::read(&aliased_output).expect("selected input retained"),
        original
    );
}

#[cfg(unix)]
#[test]
fn output_symlink_or_hard_link_to_selected_input_is_rejected_without_input_change() {
    use std::os::unix::fs::symlink;

    for target_kind in ["master", "include", "policy"] {
        for link_kind in ["symlink", "hard-link"] {
            let case = Case::new(&format!("{target_kind}-{link_kind}"));
            case.zone(&case.old, &format!("{OLD}$INCLUDE extra.inc\n"));
            case.zone(&case.new, &format!("{NEW}$INCLUDE extra.inc\n"));
            fs::write(case.old.join("extra.inc"), "included A 192.0.2.77\n").expect("old include");
            fs::write(case.new.join("extra.inc"), "included A 192.0.2.77\n").expect("new include");
            case.policy("");
            let selected_input = match target_kind {
                "master" => case.old.join("example.zone"),
                "include" => case.old.join("extra.inc"),
                "policy" => case.policy.clone(),
                _ => unreachable!(),
            };
            let original = fs::read(&selected_input).expect("original selected input");
            let aliased_output = case.root.join("aliased-report.json");
            if link_kind == "symlink" {
                symlink(&selected_input, &aliased_output).expect("output symlink");
            } else {
                fs::hard_link(&selected_input, &aliased_output).expect("output hard link");
            }

            let result = case.run_to(&aliased_output);
            assert_eq!(result.status.code(), Some(1), "{target_kind} {link_kind}");
            assert!(
                String::from_utf8_lossy(&result.stderr).contains("E_OUTPUT_ALIAS"),
                "{target_kind} {link_kind}: {}",
                String::from_utf8_lossy(&result.stderr)
            );
            assert_eq!(
                fs::read(&selected_input).expect("selected input retained"),
                original,
                "{target_kind} {link_kind}"
            );
        }
    }
}

#[test]
fn output_equal_to_policy_is_rejected_without_overwriting_policy() {
    let case = Case::new("output-is-policy");
    case.zone(&case.old, OLD);
    case.zone(&case.new, NEW);
    case.policy("");
    let original = fs::read(&case.policy).expect("original policy");

    let result = case.run_to(&case.policy);
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_OUTPUT_ALIAS"));
    assert_eq!(fs::read(&case.policy).expect("policy retained"), original);
}

#[test]
fn include_inherits_ttl_optional_origin_and_generate_expands_declared_subset() {
    let case = Case::new("include-success");
    let root_zone = concat!(
        "$ORIGIN example.test.\n$TTL 1h\n",
        "@ SOA ns hostmaster 10 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\n",
        "$INCLUDE records.inc delegated.example.test.\n",
        "$GENERATE 1-3 host$ 5m A 192.0.2.9\n",
    );
    let old_include = concat!(
        "@ NS ns\nns A 192.0.2.53\n",
        "service SRV 10 20 443 target.example.test.\n",
        "policy CAA 0 issue \"ca.example\"\n",
        "key DS 12345 13 2 AABBCCDD\n",
        "alias CNAME target.example.test.\n",
        "target AAAA 2001:db8::9\n",
    );
    let new_include = concat!(
        "@ NS ns\nns A 192.0.2.53\n",
        "service SRV 10 20 8443 target.example.test.\n",
        "policy CAA 0 issue \"new-ca.example\"\n",
        "key DS 12345 13 2 EEFF0011\n",
        "alias CNAME new-target.example.test.\n",
        "target AAAA 2001:db8::10\n",
    );
    case.zone(&case.old, root_zone);
    case.zone(
        &case.new,
        &root_zone
            .replace(" 10 ", " 11 ")
            .replace("192.0.2.9", "192.0.2.10"),
    );
    fs::write(case.old.join("records.inc"), old_include).expect("old include");
    fs::write(case.new.join("records.inc"), new_include).expect("new include");
    case.policy("");
    let result = case.run();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&case.output).expect("include report");
    assert!(report.contains("\"owner\":\"host3.example.test.\""));
    assert!(report.contains("\"type\":\"SRV\""));
    assert!(report.contains("\"type\":\"CAA\""));
    assert!(report.contains("\"type\":\"DS\""));
}

#[test]
fn maximal_and_stepped_generate_ranges_fail_bounded_without_replacing_report() {
    let cases = [
        ("max-u32", "$GENERATE 0-4294967295 host$ A 192.0.2.1\n"),
        (
            "stepped",
            "$GENERATE 0-4294967295/4294967295 host$ A 192.0.2.1\n",
        ),
    ];
    for (name, directive) in cases {
        let case = Case::new(name);
        case.zone(&case.old, OLD);
        case.zone(
            &case.new,
            &format!("$ORIGIN example.test.\n$TTL 1h\n{directive}"),
        );
        case.raw_policy(concat!(
            "max_records_total=100\nmax_records_per_zone=100\nmax_include_depth=3\n",
            "max_generate_records=1\nttl_decrease_percent=50\n",
            "zone=example.test.|example.zone|PASS|PASS\n",
        ));
        fs::write(&case.output, b"OLD-COMPLETE\n").expect("old report");
        let started = std::time::Instant::now();
        let result = case.run();
        assert!(
            started.elapsed() < std::time::Duration::from_secs(2),
            "{name}"
        );
        assert_eq!(result.status.code(), Some(1), "{name}");
        let stderr = String::from_utf8_lossy(&result.stderr);
        assert!(stderr.contains("E_GENERATE_LIMIT"), "{name}: {stderr}");
        assert!(
            !stderr.to_ascii_lowercase().contains("panicked"),
            "{name}: {stderr}"
        );
        assert_eq!(
            fs::read(&case.output).expect("previous report remains"),
            b"OLD-COMPLETE\n",
            "{name}"
        );
    }
}

#[test]
fn escaped_label_wire_length_uses_decoded_octets_and_has_exact_oracle() {
    let case = Case::new("escaped-wire-length");
    let old_label = "\\001".repeat(63);
    let new_label = "\\002".repeat(63);
    case.zone(
        &case.old,
        &minimal_zone(10, &format!("{old_label} A 192.0.2.9\n")),
    );
    case.zone(
        &case.new,
        &minimal_zone(11, &format!("{new_label} A 192.0.2.10\n")),
    );
    case.policy("");

    let result = case.run();
    assert!(
        result.status.success(),
        "stderr: {}",
        String::from_utf8_lossy(&result.stderr)
    );
    let report = fs::read_to_string(&case.output).expect("escaped-label report");
    let canonical_owner = format!("{}.example.test.", "\\\\002".repeat(63));
    let old_canonical_owner = format!("{}.example.test.", "\\\\001".repeat(63));
    assert!(
        report.contains(&format!(
            "\"kind\":\"addition\",\"owner\":\"{canonical_owner}\",\"type\":\"A\",\"rdata\":\"192.0.2.10\""
        )),
        "{report}"
    );
    assert!(
        report.contains(&format!(
            "\"kind\":\"removal\",\"owner\":\"{old_canonical_owner}\",\"type\":\"A\",\"rdata\":\"192.0.2.9\""
        )),
        "{report}"
    );
}

#[test]
fn ttl_glue_family_and_missing_soa_boundaries_match_policy() {
    let case = Case::new("ttl-drop");
    case.zone(&case.old, &minimal_zone(10, "www 1000 A 192.0.2.10\n"));
    case.zone(&case.new, &minimal_zone(11, "www 400 A 192.0.2.10\n"));
    case.policy("");
    let result = case.run();
    assert_eq!(result.status.code(), Some(2));
    let report = fs::read_to_string(&case.output).expect("TTL report");
    assert!(report.contains("TTL_DECREASE"));
    assert!(report.contains("from 1000 to 400"));

    let case = Case::new("second-family");
    let old = minimal_zone(
        10,
        "child NS ns.child\nns.child A 192.0.2.53\nns.child AAAA 2001:db8::53\n",
    );
    let new = minimal_zone(11, "child NS ns.child\nns.child AAAA 2001:db8::53\n");
    case.zone(&case.old, &old);
    case.zone(&case.new, &new);
    case.policy("");
    let result = case.run();
    assert!(result.status.success());
    let report = fs::read_to_string(&case.output).expect("glue family report");
    assert!(report.contains("GLUE_FAMILY_CHANGE"));
    assert!(!report.contains("REMOVED_LAST_GLUE"));

    let case = Case::new("missing-soa");
    case.zone(&case.old, &minimal_zone(10, ""));
    case.zone(
        &case.new,
        "$ORIGIN example.test.\n$TTL 1h\n@ NS ns\nns A 192.0.2.1\n",
    );
    case.policy("");
    let result = case.run();
    assert_eq!(result.status.code(), Some(2));
    assert!(
        fs::read_to_string(&case.output)
            .expect("apex report")
            .contains("APEX_SOA_COUNT")
    );
}

#[test]
fn overlong_escaped_label_and_total_limit_fail_closed() {
    let case = Case::new("escaped-label");
    case.zone(&case.old, OLD);
    let label = "\\097".repeat(64);
    case.zone(
        &case.new,
        &format!(
            "$ORIGIN example.test.\n$TTL 1h\n@ SOA ns hostmaster 2 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\n{label} A 192.0.2.2\n"
        ),
    );
    case.policy("");
    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_LABEL_LENGTH"));
    assert!(!case.output.exists());

    let case = Case::new("total-limit");
    case.zone(&case.old, &minimal_zone(1, ""));
    case.zone(&case.new, &minimal_zone(2, ""));
    case.raw_policy(concat!(
        "max_records_total=2\nmax_records_per_zone=10\nmax_include_depth=3\n",
        "max_generate_records=10\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|PASS|PASS\n",
    ));
    let result = case.run();
    assert_eq!(result.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&result.stderr).contains("E_TOTAL_LIMIT"));
    assert!(!case.output.exists());
}

#[test]
fn twenty_seeded_record_permutations_are_byte_identical() {
    let case = Case::new("permutations");
    let fixed = [
        "@ NS ns",
        "ns A 192.0.2.1",
        "a A 192.0.2.10",
        "b AAAA 2001:db8::20",
        "c MX 10 mail",
        "mail A 192.0.2.30",
        "d TXT \"left\" \"right\"",
        "e CAA 0 issue \"ca.example\"",
    ];
    let mut expected: Option<Vec<u8>> = None;
    for seed in 0..20_u64 {
        let mut old_lines = fixed;
        let mut new_lines = fixed;
        permute(&mut old_lines, seed.wrapping_mul(17).wrapping_add(3));
        permute(&mut new_lines, seed.wrapping_mul(31).wrapping_add(9));
        let old = format!(
            "$ORIGIN example.test.\n$TTL 1h\n@ SOA ns hostmaster 10 1h 15m 1w 5m\n{}\n",
            old_lines.join("\n")
        );
        let new = format!(
            "$TTL 3600\n$ORIGIN example.test.\n@ SOA ns hostmaster 11 3600 900 604800 300\n{}\n",
            new_lines.join("\n")
        );
        case.zone(&case.old, &old);
        case.zone(&case.new, &new);
        case.policy("");
        let result = case.run();
        assert!(
            result.status.success(),
            "seed {seed}: {}",
            String::from_utf8_lossy(&result.stderr)
        );
        let bytes = fs::read(&case.output).expect("permuted report");
        if let Some(expected) = &expected {
            assert_eq!(&bytes, expected, "seed {seed}");
        } else {
            expected = Some(bytes);
        }
    }
}

#[test]
fn total_expansion_limit_is_applied_independently_to_each_snapshot() {
    let case = Case::new("snapshot-total");
    case.zone(&case.old, &minimal_zone(1, ""));
    case.zone(&case.new, &minimal_zone(1, ""));
    case.raw_policy(concat!(
        "max_records_total=3\nmax_records_per_zone=3\nmax_include_depth=3\n",
        "max_generate_records=10\nttl_decrease_percent=50\n",
        "zone=example.test.|example.zone|PASS|PASS\n",
    ));
    let result = case.run();
    assert!(
        result.status.success(),
        "each three-record snapshot is within the cap: {}",
        String::from_utf8_lossy(&result.stderr)
    );
}

fn minimal_zone(serial: u32, extra: &str) -> String {
    format!(
        "$ORIGIN example.test.\n$TTL 1h\n@ SOA ns hostmaster {serial} 1h 15m 1w 5m\n@ NS ns\nns A 192.0.2.1\n{extra}"
    )
}

fn permute<const N: usize>(values: &mut [&str; N], mut state: u64) {
    for index in (1..N).rev() {
        state = state
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1);
        let other = usize::try_from(state % u64::try_from(index + 1).expect("index width"))
            .expect("position width");
        values.swap(index, other);
    }
}

fn publication_temp_entries(parent: &Path) -> Vec<PathBuf> {
    let mut entries = fs::read_dir(parent)
        .expect("read output parent")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.file_name()
                .and_then(std::ffi::OsStr::to_str)
                .is_some_and(|name| name.starts_with(".report.json.tmp-"))
        })
        .collect::<Vec<_>>();
    entries.sort();
    entries
}
