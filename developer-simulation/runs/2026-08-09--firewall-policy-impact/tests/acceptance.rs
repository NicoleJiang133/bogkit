use firewall_policy_impact::{
    Action, ChangeKind, IpFamily, Packet, PolicyFile, Protocol, RuleInput, analyze,
    analyze_files_to_report, evaluate_linear, parse_and_validate, region_contains, report_json,
    validation_suite, verify_report,
};
use std::sync::atomic::{AtomicU64, Ordering};

fn rule(
    id: &str,
    action: Action,
    family: IpFamily,
    cidr: &str,
    protocol: Protocol,
    ports: (u16, u16),
) -> RuleInput {
    RuleInput {
        id: id.to_string(),
        action,
        ip_family: family,
        source_cidr: cidr.to_string(),
        protocol,
        port_start: ports.0,
        port_end: ports.1,
    }
}

fn policy(rules: Vec<RuleInput>) -> firewall_policy_impact::ValidatedPolicy {
    let bytes = serde_json::to_vec(&PolicyFile { rules }).unwrap();
    parse_and_validate(&bytes, "test").unwrap()
}

fn packet(family: IpFamily, source: u128, protocol: Protocol, port: u16) -> Packet {
    Packet {
        family,
        source,
        protocol,
        destination_port: port,
    }
}

#[test]
fn hand_written_first_match_regressions() {
    let cases = vec![
        (
            "insertion before allow",
            policy(vec![rule(
                "allow-all",
                Action::Allow,
                IpFamily::Ipv4,
                "0.0.0.0/0",
                Protocol::Tcp,
                (0, 65535),
            )]),
            policy(vec![
                rule(
                    "inserted-deny",
                    Action::Deny,
                    IpFamily::Ipv4,
                    "10.0.0.0/8",
                    Protocol::Tcp,
                    (443, 443),
                ),
                rule(
                    "allow-all",
                    Action::Allow,
                    IpFamily::Ipv4,
                    "0.0.0.0/0",
                    Protocol::Tcp,
                    (0, 65535),
                ),
            ]),
            ChangeKind::NewlyDenied,
        ),
        (
            "deletion of deny",
            policy(vec![
                rule(
                    "deny",
                    Action::Deny,
                    IpFamily::Ipv4,
                    "10.0.0.0/8",
                    Protocol::Tcp,
                    (0, 1023),
                ),
                rule(
                    "allow",
                    Action::Allow,
                    IpFamily::Ipv4,
                    "0.0.0.0/0",
                    Protocol::Tcp,
                    (0, 65535),
                ),
            ]),
            policy(vec![rule(
                "allow",
                Action::Allow,
                IpFamily::Ipv4,
                "0.0.0.0/0",
                Protocol::Tcp,
                (0, 65535),
            )]),
            ChangeKind::NewlyAllowed,
        ),
        (
            "rule reordering",
            policy(vec![
                rule(
                    "allow-narrow",
                    Action::Allow,
                    IpFamily::Ipv4,
                    "10.2.0.0/16",
                    Protocol::Udp,
                    (50, 100),
                ),
                rule(
                    "deny-broad",
                    Action::Deny,
                    IpFamily::Ipv4,
                    "10.0.0.0/8",
                    Protocol::Udp,
                    (0, 200),
                ),
            ]),
            policy(vec![
                rule(
                    "deny-broad",
                    Action::Deny,
                    IpFamily::Ipv4,
                    "10.0.0.0/8",
                    Protocol::Udp,
                    (0, 200),
                ),
                rule(
                    "allow-narrow",
                    Action::Allow,
                    IpFamily::Ipv4,
                    "10.2.0.0/16",
                    Protocol::Udp,
                    (50, 100),
                ),
            ]),
            ChangeKind::NewlyDenied,
        ),
        (
            "partial CIDR and port overlap",
            policy(vec![rule(
                "allow-old",
                Action::Allow,
                IpFamily::Ipv4,
                "192.0.2.0/25",
                Protocol::Tcp,
                (100, 200),
            )]),
            policy(vec![rule(
                "allow-new",
                Action::Allow,
                IpFamily::Ipv4,
                "192.0.2.0/24",
                Protocol::Tcp,
                (150, 250),
            )]),
            ChangeKind::NewlyDenied,
        ),
        (
            "default deny transition",
            policy(vec![]),
            policy(vec![rule(
                "new-allow",
                Action::Allow,
                IpFamily::Ipv6,
                "2001:db8::/126",
                Protocol::Tcp,
                (8443, 8443),
            )]),
            ChangeKind::NewlyAllowed,
        ),
    ];

    for (name, old, proposed, expected_kind) in cases {
        let report = analyze(&old, &proposed);
        assert!(!report.changes.is_empty(), "{name}");
        assert!(
            report
                .changes
                .iter()
                .any(|change| change.kind == expected_kind),
            "{name}"
        );
        if name == "partial CIDR and port overlap" {
            assert_eq!(report.summary.outcome, "newly-allowed-and-newly-denied");
            assert!(
                report
                    .changes
                    .iter()
                    .any(|change| change.kind == ChangeKind::NewlyAllowed)
            );
        }
        verify_report(&old, &proposed, &report).unwrap();
    }
}

#[test]
fn shadowing_reachability_and_semantically_neutral_edits() {
    let old = policy(vec![
        rule(
            "broad",
            Action::Allow,
            IpFamily::Ipv4,
            "10.0.0.0/8",
            Protocol::Tcp,
            (0, 65535),
        ),
        rule(
            "same-shadow",
            Action::Allow,
            IpFamily::Ipv4,
            "10.1.0.0/16",
            Protocol::Tcp,
            (80, 80),
        ),
        rule(
            "opposite-shadow",
            Action::Deny,
            IpFamily::Ipv4,
            "10.2.0.0/16",
            Protocol::Tcp,
            (443, 443),
        ),
    ]);
    let proposed = policy(vec![
        rule(
            "broad",
            Action::Allow,
            IpFamily::Ipv4,
            "10.0.0.0/8",
            Protocol::Tcp,
            (0, 65535),
        ),
        rule(
            "same-shadow-edited",
            Action::Allow,
            IpFamily::Ipv4,
            "10.1.0.0/24",
            Protocol::Tcp,
            (80, 81),
        ),
        rule(
            "opposite-shadow",
            Action::Deny,
            IpFamily::Ipv4,
            "10.2.0.0/16",
            Protocol::Tcp,
            (443, 443),
        ),
    ]);
    let report = analyze(&old, &proposed);
    assert_eq!(report.summary.outcome, "exact-no-semantic-change");
    assert!(report.changes.is_empty());
    assert!(report.proposed_rule_reachability[0].reachable);
    assert!(!report.proposed_rule_reachability[1].reachable);
    assert!(!report.proposed_rule_reachability[2].reachable);
    verify_report(&old, &proposed, &report).unwrap();
}

#[test]
fn families_protocols_and_extreme_ports_remain_separate() {
    let old = policy(vec![]);
    let proposed = policy(vec![
        rule(
            "v4-zero",
            Action::Allow,
            IpFamily::Ipv4,
            "203.0.113.9/32",
            Protocol::Tcp,
            (0, 0),
        ),
        rule(
            "v6-max",
            Action::Allow,
            IpFamily::Ipv6,
            "2001:db8::1/128",
            Protocol::Udp,
            (65535, 65535),
        ),
    ]);
    let report = analyze(&old, &proposed);
    assert_eq!(report.changes.len(), 2);
    for expected in [
        packet(
            IpFamily::Ipv4,
            u32::from(std::net::Ipv4Addr::new(203, 0, 113, 9)) as u128,
            Protocol::Tcp,
            0,
        ),
        packet(
            IpFamily::Ipv6,
            u128::from("2001:db8::1".parse::<std::net::Ipv6Addr>().unwrap()),
            Protocol::Udp,
            65535,
        ),
    ] {
        assert_eq!(
            report
                .changes
                .iter()
                .filter(|region| region_contains(region, expected))
                .count(),
            1
        );
    }
    let wrong_family = packet(
        IpFamily::Ipv6,
        u128::from("::cb00:7109".parse::<std::net::Ipv6Addr>().unwrap()),
        Protocol::Tcp,
        0,
    );
    assert_eq!(
        evaluate_linear(&proposed, wrong_family).action,
        Action::Deny
    );
    verify_report(&old, &proposed, &report).unwrap();
}

#[test]
fn malformed_inputs_are_rejected_before_analysis() {
    let malformed = [
        r#"{"rules":[{"id":"x","action":"permit","ip_family":"ipv4","source_cidr":"0.0.0.0/0","protocol":"tcp","port_start":0,"port_end":1}]}"#,
        r#"{"rules":[{"id":"x","action":"allow","ip_family":"ipv4","source_cidr":"0.0.0.0/0","protocol":"icmp","port_start":0,"port_end":1}]}"#,
        r#"{"rules":[{"id":"x","action":"allow","ip_family":"ipv4","source_cidr":"10.0.0.1/24","protocol":"tcp","port_start":0,"port_end":1}]}"#,
        r#"{"rules":[{"id":"x","action":"allow","ip_family":"ipv4","source_cidr":"2001:db8::/32","protocol":"tcp","port_start":0,"port_end":1}]}"#,
        r#"{"rules":[{"id":"x","action":"allow","ip_family":"ipv4","source_cidr":"0.0.0.0/0","protocol":"tcp","port_start":2,"port_end":1}]}"#,
        r#"{"rules":[{"id":"x","action":"allow","ip_family":"ipv4","source_cidr":"0.0.0.0/0","protocol":"tcp","port_start":0,"port_end":1},{"id":"x","action":"deny","ip_family":"ipv4","source_cidr":"10.0.0.0/8","protocol":"udp","port_start":0,"port_end":1}]}"#,
    ];
    for input in malformed {
        assert!(
            parse_and_validate(input.as_bytes(), "bad").is_err(),
            "{input}"
        );
    }
}

#[test]
fn implicit_default_marker_cannot_be_used_as_a_real_rule_id() {
    let input = r#"{"rules":[{"id":"default-deny","action":"allow","ip_family":"ipv4","source_cidr":"0.0.0.0/0","protocol":"tcp","port_start":0,"port_end":65535}]}"#;
    let errors = parse_and_validate(input.as_bytes(), "adversarial policy").unwrap_err();
    assert_eq!(errors.len(), 1);
    assert!(errors[0].contains("reserved for the implicit default-deny decision"));
}

#[test]
fn reused_output_is_removed_on_validation_read_and_write_failures() {
    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::current_dir().unwrap().join(format!(
        ".reused-output-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let old_path = directory.join("old.json");
    let proposed_path = directory.join("proposed.json");
    let report_path = directory.join("report.json");
    let valid = serde_json::to_vec(&PolicyFile {
        rules: vec![rule(
            "allow-web",
            Action::Allow,
            IpFamily::Ipv4,
            "10.0.0.0/24",
            Protocol::Tcp,
            (443, 443),
        )],
    })
    .unwrap();
    let invalid = br#"{"rules":[{"id":"bad","action":"allow","ip_family":"ipv4","source_cidr":"10.0.0.1/24","protocol":"tcp","port_start":443,"port_end":443}]}"#;

    std::fs::write(&old_path, &valid).unwrap();
    std::fs::write(&proposed_path, &valid).unwrap();
    analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();
    assert!(report_path.is_file());

    std::fs::write(&proposed_path, invalid).unwrap();
    let validation_error =
        analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap_err();
    assert!(validation_error.contains("no semantic verdict was produced"));
    assert!(!report_path.exists());

    std::fs::write(&proposed_path, &valid).unwrap();
    analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();
    assert!(report_path.is_file());
    std::fs::remove_file(&proposed_path).unwrap();
    let read_error = analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap_err();
    assert!(read_error.contains("cannot read"));
    assert!(!report_path.exists());

    std::fs::write(&proposed_path, &valid).unwrap();
    let missing_parent_report = directory.join("missing-parent/report.json");
    let write_error =
        analyze_files_to_report(&old_path, &proposed_path, &missing_parent_report).unwrap_err();
    assert!(write_error.contains("cannot create temporary report"));
    assert!(!missing_parent_report.exists());

    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn report_schema_rejects_unknown_fields_at_every_nested_level() {
    let old = policy(vec![]);
    let proposed = policy(vec![rule(
        "allow-web",
        Action::Allow,
        IpFamily::Ipv4,
        "10.0.0.0/24",
        Protocol::Tcp,
        (443, 443),
    )]);
    let report = analyze(&old, &proposed);
    let original = serde_json::to_value(report).unwrap();

    for pointer in [
        "",
        "/summary",
        "/changes/0",
        "/changes/0/witness",
        "/proposed_rule_reachability/0",
        "/proposed_rule_reachability/0/witness",
    ] {
        let mut corrupted = original.clone();
        let object = corrupted
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap();
        object.insert("unexpected".to_string(), serde_json::json!(true));
        assert!(
            serde_json::from_value::<firewall_policy_impact::Report>(corrupted).is_err(),
            "unknown field accepted at JSON pointer {pointer:?}"
        );
    }
}

#[test]
fn output_aliases_are_rejected_before_either_input_is_changed() {
    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    let directory_name = format!(
        ".output-alias-test-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    );
    let directory = std::env::current_dir().unwrap().join(&directory_name);
    std::fs::create_dir(&directory).unwrap();
    std::fs::create_dir(directory.join("subdirectory")).unwrap();
    let old_path = directory.join("old.json");
    let proposed_path = directory.join("proposed.json");
    let old_bytes = serde_json::to_vec(&PolicyFile {
        rules: vec![rule(
            "old-allow",
            Action::Allow,
            IpFamily::Ipv4,
            "10.0.0.0/24",
            Protocol::Tcp,
            (443, 443),
        )],
    })
    .unwrap();
    let proposed_bytes = serde_json::to_vec(&PolicyFile {
        rules: vec![rule(
            "proposed-deny",
            Action::Deny,
            IpFamily::Ipv4,
            "10.0.0.0/24",
            Protocol::Tcp,
            (443, 443),
        )],
    })
    .unwrap();
    std::fs::write(&old_path, &old_bytes).unwrap();
    std::fs::write(&proposed_path, &proposed_bytes).unwrap();

    let relative_alias = std::path::PathBuf::from(&directory_name)
        .join("subdirectory")
        .join("..")
        .join("old.json");
    let mut aliases = vec![old_path.clone(), proposed_path.clone(), relative_alias];

    #[cfg(unix)]
    {
        let symlink_alias = directory.join("old-symlink.json");
        std::os::unix::fs::symlink(&old_path, &symlink_alias).unwrap();
        aliases.push(symlink_alias);

        let hard_link_alias = directory.join("old-hard-link.json");
        std::fs::hard_link(&old_path, &hard_link_alias).unwrap();
        aliases.push(hard_link_alias);
    }

    for alias in aliases {
        let error = analyze_files_to_report(&old_path, &proposed_path, &alias).unwrap_err();
        assert!(error.contains("aliases the"), "unexpected error: {error}");
        assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
        assert_eq!(std::fs::read(&proposed_path).unwrap(), proposed_bytes);
        assert!(old_path.is_file());
        assert!(proposed_path.is_file());
    }

    std::fs::remove_dir_all(&directory).unwrap();
}

fn publication_fixture(
    label: &str,
) -> (
    std::path::PathBuf,
    std::path::PathBuf,
    std::path::PathBuf,
    Vec<u8>,
    Vec<u8>,
) {
    static NEXT_DIRECTORY: AtomicU64 = AtomicU64::new(0);
    let directory = std::env::current_dir().unwrap().join(format!(
        ".publication-test-{label}-{}-{}",
        std::process::id(),
        NEXT_DIRECTORY.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir(&directory).unwrap();
    let old_path = directory.join("old.json");
    let proposed_path = directory.join("proposed.json");
    let old_bytes = serde_json::to_vec(&PolicyFile {
        rules: vec![rule(
            "old-deny",
            Action::Deny,
            IpFamily::Ipv4,
            "10.0.0.0/24",
            Protocol::Tcp,
            (443, 443),
        )],
    })
    .unwrap();
    let proposed_bytes = serde_json::to_vec(&PolicyFile {
        rules: vec![rule(
            "proposed-allow",
            Action::Allow,
            IpFamily::Ipv4,
            "10.0.0.0/24",
            Protocol::Tcp,
            (443, 443),
        )],
    })
    .unwrap();
    std::fs::write(&old_path, &old_bytes).unwrap();
    std::fs::write(&proposed_path, &proposed_bytes).unwrap();
    (
        directory,
        old_path,
        proposed_path,
        old_bytes,
        proposed_bytes,
    )
}

fn temporary_candidate(directory: &std::path::Path, attempt: usize) -> std::path::PathBuf {
    let suffix = if attempt == 0 {
        String::new()
    } else {
        format!("-{attempt}")
    };
    directory.join(format!(".report.json.tmp-{}{suffix}", std::process::id()))
}

#[test]
fn preexisting_temporary_hard_link_never_truncates_an_input() {
    let (directory, old_path, proposed_path, old_bytes, proposed_bytes) =
        publication_fixture("hard-link");
    let report_path = directory.join("report.json");
    let collision = temporary_candidate(&directory, 0);
    std::fs::hard_link(&old_path, &collision).unwrap();

    analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();

    assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
    assert_eq!(std::fs::read(&proposed_path).unwrap(), proposed_bytes);
    assert_eq!(std::fs::read(&collision).unwrap(), old_bytes);
    assert!(report_path.is_file());
    assert!(!temporary_candidate(&directory, 1).exists());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[cfg(unix)]
#[test]
fn preexisting_temporary_symlink_never_truncates_an_input() {
    let (directory, old_path, proposed_path, old_bytes, proposed_bytes) =
        publication_fixture("symlink");
    let report_path = directory.join("report.json");
    let collision = temporary_candidate(&directory, 0);
    std::os::unix::fs::symlink(&old_path, &collision).unwrap();

    analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();

    assert_eq!(std::fs::read(&old_path).unwrap(), old_bytes);
    assert_eq!(std::fs::read(&proposed_path).unwrap(), proposed_bytes);
    assert_eq!(std::fs::read(&collision).unwrap(), old_bytes);
    assert!(
        std::fs::symlink_metadata(&collision)
            .unwrap()
            .file_type()
            .is_symlink()
    );
    assert!(report_path.is_file());
    assert!(!temporary_candidate(&directory, 1).exists());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn temporary_collisions_are_retried_without_deleting_foreign_candidates() {
    let (directory, old_path, proposed_path, _, _) = publication_fixture("retry");
    let report_path = directory.join("report.json");
    let first = temporary_candidate(&directory, 0);
    let second = temporary_candidate(&directory, 1);
    std::fs::write(&first, b"foreign collision zero").unwrap();
    std::fs::write(&second, b"foreign collision one").unwrap();

    analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();

    assert_eq!(std::fs::read(&first).unwrap(), b"foreign collision zero");
    assert_eq!(std::fs::read(&second).unwrap(), b"foreign collision one");
    assert!(!temporary_candidate(&directory, 2).exists());
    assert!(report_path.is_file());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn normal_atomic_publication_leaves_no_temporary_file() {
    let (directory, old_path, proposed_path, _, _) = publication_fixture("normal");
    let report_path = directory.join("report.json");

    let first = analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();
    let first_bytes = std::fs::read(&report_path).unwrap();
    let second = analyze_files_to_report(&old_path, &proposed_path, &report_path).unwrap();
    let second_bytes = std::fs::read(&report_path).unwrap();

    assert_eq!(first, second);
    assert_eq!(first_bytes, second_bytes);
    assert_eq!(first_bytes, report_json(&first).unwrap());
    assert!(!temporary_candidate(&directory, 0).exists());
    std::fs::remove_dir_all(&directory).unwrap();
}

#[test]
fn output_is_byte_deterministic_and_sorted() {
    let old = policy(vec![
        rule(
            "old-v6",
            Action::Allow,
            IpFamily::Ipv6,
            "::/0",
            Protocol::Udp,
            (0, 65535),
        ),
        rule(
            "old-v4",
            Action::Allow,
            IpFamily::Ipv4,
            "0.0.0.0/0",
            Protocol::Tcp,
            (0, 65535),
        ),
    ]);
    let proposed = policy(vec![]);
    let first = report_json(&analyze(&old, &proposed)).unwrap();
    let second = report_json(&analyze(&old, &proposed)).unwrap();
    assert_eq!(first, second);
    let report = analyze(&old, &proposed);
    assert_eq!(report.changes[0].ip_family, IpFamily::Ipv4);
    assert_eq!(report.changes[1].ip_family, IpFamily::Ipv6);
}

#[test]
fn acceptance_differential_workloads() {
    let result = validation_suite(10_000, 1_000, 2_000_000, 0x5eed_2026_0809).unwrap();
    println!("{result}");
    assert!(result.contains("small_pairs=10000"));
    assert!(result.contains("full_width_pairs=1000"));
    assert!(result.contains("boundary_probes=2000000"));
    assert!(result.contains("disagreements=0"));
}
