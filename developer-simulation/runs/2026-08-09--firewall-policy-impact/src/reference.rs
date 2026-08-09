use crate::{Action, Decision, IpFamily, Packet, Protocol, ValidatedPolicy};

/// Intentionally simple O(rules) first-match evaluator.
///
/// It shares only validated data types with the analyzer, not its sweep-line
/// implementation. Tests and the `verify` command use it as an independent
/// replay oracle for witnesses and probes.
pub fn evaluate(policy: &ValidatedPolicy, packet: Packet) -> Decision {
    for rule in &policy.rules {
        if rule.family == packet.family
            && rule.protocol == packet.protocol
            && rule.source_start <= packet.source
            && packet.source <= rule.source_end
            && rule.port_start <= packet.destination_port
            && packet.destination_port <= rule.port_end
        {
            return Decision {
                action: rule.action,
                rule_id: rule.id.clone(),
            };
        }
    }

    Decision {
        action: Action::Deny,
        rule_id: "default-deny".to_string(),
    }
}

pub fn parse_packet(
    source_ip: &str,
    protocol: Protocol,
    destination_port: u16,
) -> Result<Packet, String> {
    let ip: std::net::IpAddr = source_ip
        .parse()
        .map_err(|_| format!("invalid packet source_ip {source_ip:?}"))?;
    let (family, source) = match ip {
        std::net::IpAddr::V4(ip) => (IpFamily::Ipv4, u32::from(ip) as u128),
        std::net::IpAddr::V6(ip) => (IpFamily::Ipv6, u128::from(ip)),
    };
    Ok(Packet {
        family,
        source,
        protocol,
        destination_port,
    })
}
