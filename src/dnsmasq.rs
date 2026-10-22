use std::collections::HashSet;
use std::sync::LazyLock;

use regex::Regex;

pub struct StaticHost {
    pub ip: String,
    pub mac: String,
    pub hostname: Option<String>,
}

/// Replaces all `dhcp-host=` lines with the given static leases, keeping everything else.
pub fn render_conf(old: &str, hosts: &[StaticHost]) -> String {
    let mut conf: String = old
        .trim()
        .split('\n')
        .filter(|line| !line.starts_with("dhcp-host="))
        .map(|line| format!("{line}\n"))
        .collect();

    for host in hosts {
        conf.push_str(&format!("dhcp-host={},set:client_is_a_pi,{}", host.mac, host.ip));
        if let Some(hostname) = host.hostname.as_deref().filter(|h| !h.is_empty()) {
            conf.push_str(&format!(",{hostname}"));
        }
        conf.push('\n');
    }
    conf
}

/// First address in 172.20.0.0/16 not in use, skipping the network address and the server's 172.20.0.1.
pub fn first_available_ip(used: &HashSet<String>) -> Option<String> {
    (0..=255u8)
        .flat_map(|i| (0..=255u8).map(move |j| (i, j)))
        .filter(|&(i, j)| !(i == 0 && j <= 1))
        .map(|(i, j)| format!("172.20.{i}.{j}"))
        .find(|ip| !used.contains(ip))
}

static NO_ADDRESS: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r" ([0-9A-Fa-f:]{17}) no address available").unwrap());

/// MAC addresses that dnsmasq refused a lease, in order of first appearance.
pub fn macs_without_lease(log: &str) -> Vec<String> {
    let mut seen = HashSet::new();
    NO_ADDRESS
        .captures_iter(log)
        .map(|c| c[1].to_string())
        .filter(|mac| seen.insert(mac.clone()))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONF: &str = include_str!("../etc/dnsmasq.conf");

    fn host(ip: &str, mac: &str, hostname: Option<&str>) -> StaticHost {
        StaticHost { ip: ip.into(), mac: mac.into(), hostname: hostname.map(Into::into) }
    }

    #[test]
    fn appends_hosts_and_replaces_previous_ones() {
        let first = render_conf(CONF, &[host("172.20.0.2", "dc:a6:32:00:00:01", None)]);
        let second = render_conf(&first, &[host("172.20.0.3", "dc:a6:32:00:00:02", Some("cm4-a"))]);

        assert_eq!(second, format!("{}\ndhcp-host=dc:a6:32:00:00:02,set:client_is_a_pi,172.20.0.3,cm4-a\n", CONF.trim()));
    }

    #[test]
    fn without_hosts_only_normalizes_whitespace() {
        assert_eq!(render_conf(CONF, &[]), format!("{}\n", CONF.trim()));
    }

    #[test]
    fn skips_network_and_server_address() {
        assert_eq!(first_available_ip(&HashSet::new()).as_deref(), Some("172.20.0.2"));
    }

    #[test]
    fn skips_used_addresses_across_octets() {
        let used: HashSet<String> = (2..=255).map(|j| format!("172.20.0.{j}")).collect();
        assert_eq!(first_available_ip(&used).as_deref(), Some("172.20.1.0"));
    }

    #[test]
    fn exhausted_range() {
        let used: HashSet<String> = (0..=255).flat_map(|i| (0..=255).map(move |j| format!("172.20.{i}.{j}"))).collect();
        assert_eq!(first_available_ip(&used), None);
    }

    #[test]
    fn finds_refused_macs_once_in_order() {
        let log = "\
Oct 21 dnsmasq-dhcp[1]: DHCPDISCOVER(eth0) dc:a6:32:00:00:02 no address available
Oct 21 dnsmasq-dhcp[1]: DHCPDISCOVER(eth0) dc:a6:32:00:00:01 no address available
Oct 21 dnsmasq-dhcp[1]: DHCPDISCOVER(eth0) dc:a6:32:00:00:02 no address available
Oct 21 dnsmasq-dhcp[1]: DHCPOFFER(eth0) 172.20.0.9 dc:a6:32:00:00:03";
        assert_eq!(macs_without_lease(log), ["dc:a6:32:00:00:02", "dc:a6:32:00:00:01"]);
    }
}
