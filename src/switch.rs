//! Finds which switch port a MAC address was learned on, via BRIDGE-MIB over SNMP v2c.

use std::collections::btree_map::Entry;
use std::collections::{BTreeMap, HashMap};
use std::time::Duration;

use snmp2::{Oid, SyncSession, Value};

const MACTABLE: &[u64] = &[1, 3, 6, 1, 2, 1, 17, 4, 3]; // BRIDGE-MIB::dot1dTpFdbTable
const MACTABLE_MAC: &[u64] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 1]; // dot1dTpFdbAddress
const MACTABLE_PORT: &[u64] = &[1, 3, 6, 1, 2, 1, 17, 4, 3, 1, 2]; // dot1dTpFdbPort
const BASEPORT_IFINDEX: &[u64] = &[1, 3, 6, 1, 2, 1, 17, 1, 4, 1, 2]; // dot1dBasePortIfIndex
const IFNAME: &[u64] = &[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 1];
const IFALIAS: &[u64] = &[1, 3, 6, 1, 2, 1, 31, 1, 1, 1, 18];

pub const MULTIPLE_PORTS: &str = "[multiple ports]";

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SnmpValue {
    Integer(i64),
    Bytes(Vec<u8>),
    Other,
}

impl SnmpValue {
    fn integer(&self) -> Option<i64> {
        match self {
            SnmpValue::Integer(i) => Some(*i),
            _ => None,
        }
    }

    fn text(&self) -> Option<String> {
        match self {
            SnmpValue::Bytes(b) => Some(String::from_utf8_lossy(b).into_owned()),
            SnmpValue::Integer(i) => Some(i.to_string()),
            SnmpValue::Other => None,
        }
    }
}

pub trait Snmp {
    fn get(&mut self, oid: &[u64]) -> Option<SnmpValue>;
    /// All values below `oid`, or None when the agent could not be queried at all.
    fn walk(&mut self, oid: &[u64]) -> Option<Vec<(Vec<u64>, SnmpValue)>>;
}

pub struct EthernetSwitch<S> {
    snmp: S,
    baseport_ifindex: HashMap<i64, Option<i64>>,
    ifindex_name: HashMap<i64, Option<String>>,
}

enum Port {
    Base(i64),
    Multiple,
}

impl<S: Snmp> EthernetSwitch<S> {
    pub fn new(snmp: S) -> Self {
        Self { snmp, baseport_ifindex: HashMap::new(), ifindex_name: HashMap::new() }
    }

    pub fn port_name_by_mac(&mut self, mac: &str) -> Option<String> {
        let table = self.mac_to_baseport()?;
        match table.get(mac)? {
            Port::Multiple => Some(MULTIPLE_PORTS.to_string()),
            Port::Base(port) => self.port_name_by_baseport(*port),
        }
    }

    pub fn mac_to_port_name(&mut self) -> Option<BTreeMap<String, String>> {
        let table = self.mac_to_baseport()?;
        Some(
            table
                .into_iter()
                .map(|(mac, port)| {
                    let name = match port {
                        Port::Multiple => Some(MULTIPLE_PORTS.to_string()),
                        Port::Base(port) => self.port_name_by_baseport(port),
                    };
                    (mac, name.unwrap_or_default())
                })
                .collect(),
        )
    }

    fn mac_to_baseport(&mut self) -> Option<BTreeMap<String, Port>> {
        let rows = self.snmp.walk(MACTABLE)?;
        if rows.is_empty() {
            return None;
        }

        let mut macs = Vec::new();
        let mut ports = HashMap::new();
        for (oid, value) in rows {
            if let Some(index) = oid.strip_prefix(MACTABLE_MAC) {
                if let SnmpValue::Bytes(bytes) = value {
                    macs.push((index.to_vec(), format_mac(&bytes)?));
                }
            } else if let Some(index) = oid.strip_prefix(MACTABLE_PORT) {
                ports.insert(index.to_vec(), value.integer());
            }
        }

        let mut table = BTreeMap::new();
        for (index, mac) in macs {
            match table.entry(mac) {
                Entry::Occupied(mut seen) => {
                    seen.insert(Port::Multiple);
                }
                Entry::Vacant(new) => {
                    if let Some(port) = ports.get(&index) {
                        // A zero or missing port is unusable, which the PHP code reported as multiple ports.
                        new.insert(port.filter(|&p| p != 0).map_or(Port::Multiple, Port::Base));
                    }
                }
            }
        }
        Some(table)
    }

    fn port_name_by_baseport(&mut self, baseport: i64) -> Option<String> {
        let ifindex = self.ifindex_by_baseport(baseport)?;
        self.port_name_by_ifindex(ifindex)
    }

    fn ifindex_by_baseport(&mut self, baseport: i64) -> Option<i64> {
        if let Some(cached) = self.baseport_ifindex.get(&baseport) {
            return *cached;
        }
        let ifindex = self.snmp.get(&[BASEPORT_IFINDEX, &[baseport as u64]].concat()).and_then(|v| v.integer());
        self.baseport_ifindex.insert(baseport, ifindex);
        ifindex
    }

    /// Prefers the user configured ifAlias over the system defined ifName.
    fn port_name_by_ifindex(&mut self, ifindex: i64) -> Option<String> {
        if let Some(cached) = self.ifindex_name.get(&ifindex) {
            return cached.clone();
        }
        let index = [ifindex as u64];
        let name = self
            .snmp
            .get(&[IFALIAS, &index].concat())
            .and_then(|v| v.text())
            .filter(|alias| !alias.is_empty())
            .or_else(|| self.snmp.get(&[IFNAME, &index].concat()).and_then(|v| v.text()));
        self.ifindex_name.insert(ifindex, name.clone());
        name
    }
}

fn format_mac(bytes: &[u8]) -> Option<String> {
    if bytes.len() != 6 {
        return None;
    }
    Some(bytes.iter().map(|b| format!("{b:02x}")).collect::<Vec<_>>().join(":"))
}

pub struct SnmpSession {
    session: SyncSession,
    retries: usize,
}

impl SnmpSession {
    pub fn connect(ip: &str, community: &str) -> std::io::Result<Self> {
        let session = SyncSession::new_v2c((ip, 161), community.as_bytes(), Some(Duration::from_secs(1)), 0)?;
        Ok(Self { session, retries: 1 })
    }

    fn getnext(&mut self, oid: &[u64]) -> Option<(Vec<u64>, SnmpValue)> {
        let oid = Oid::from(oid).ok()?;
        for _ in 0..=self.retries {
            if let Ok(mut pdu) = self.session.getnext(&oid) {
                let (oid, value) = pdu.varbinds.next()?;
                return Some((oid.iter()?.collect(), convert(value)?));
            }
        }
        None
    }
}

fn convert(value: Value) -> Option<SnmpValue> {
    Some(match value {
        Value::Integer(i) => SnmpValue::Integer(i),
        Value::Counter32(i) | Value::Unsigned32(i) | Value::Timeticks(i) => SnmpValue::Integer(i.into()),
        Value::OctetString(b) => SnmpValue::Bytes(b.to_vec()),
        Value::EndOfMibView | Value::NoSuchObject | Value::NoSuchInstance => return None,
        _ => SnmpValue::Other,
    })
}

impl Snmp for SnmpSession {
    fn get(&mut self, oid: &[u64]) -> Option<SnmpValue> {
        let oid = Oid::from(oid).ok()?;
        for _ in 0..=self.retries {
            if let Ok(mut pdu) = self.session.get(&oid) {
                return convert(pdu.varbinds.next()?.1);
            }
        }
        None
    }

    fn walk(&mut self, root: &[u64]) -> Option<Vec<(Vec<u64>, SnmpValue)>> {
        let mut rows = Vec::new();
        let mut current = root.to_vec();
        loop {
            match self.getnext(&current) {
                Some((oid, value)) if oid.starts_with(root) && oid > current => {
                    current = oid.clone();
                    rows.push((oid, value));
                }
                Some(_) => return Some(rows),
                None if rows.is_empty() => return None,
                None => return Some(rows),
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Default)]
    struct FakeSnmp {
        values: BTreeMap<Vec<u64>, SnmpValue>,
        gets: usize,
    }

    impl FakeSnmp {
        fn set(&mut self, prefix: &[u64], index: &[u64], value: SnmpValue) {
            self.values.insert([prefix, index].concat(), value);
        }

        fn fdb(&mut self, mac: [u8; 6], port: i64) {
            let index: Vec<u64> = mac.iter().map(|&b| b.into()).collect();
            self.set(MACTABLE_MAC, &index, SnmpValue::Bytes(mac.to_vec()));
            self.set(MACTABLE_PORT, &index, SnmpValue::Integer(port));
        }
    }

    impl Snmp for &mut FakeSnmp {
        fn get(&mut self, oid: &[u64]) -> Option<SnmpValue> {
            self.gets += 1;
            self.values.get(oid).cloned()
        }

        fn walk(&mut self, oid: &[u64]) -> Option<Vec<(Vec<u64>, SnmpValue)>> {
            Some(self.values.iter().filter(|(k, _)| k.starts_with(oid)).map(|(k, v)| (k.clone(), v.clone())).collect())
        }
    }

    fn switch() -> FakeSnmp {
        let mut snmp = FakeSnmp::default();
        snmp.fdb([0xdc, 0xa6, 0x32, 0, 0, 1], 1);
        snmp.fdb([0xdc, 0xa6, 0x32, 0, 0, 2], 2);
        snmp.set(BASEPORT_IFINDEX, &[1], SnmpValue::Integer(101));
        snmp.set(BASEPORT_IFINDEX, &[2], SnmpValue::Integer(102));
        snmp.set(IFNAME, &[101], SnmpValue::Bytes(b"ge-0/0/1".to_vec()));
        snmp.set(IFALIAS, &[101], SnmpValue::Bytes(Vec::new()));
        snmp.set(IFNAME, &[102], SnmpValue::Bytes(b"ge-0/0/2".to_vec()));
        snmp.set(IFALIAS, &[102], SnmpValue::Bytes(b"jig 2".to_vec()));
        snmp
    }

    #[test]
    fn falls_back_to_ifname_without_alias() {
        let mut snmp = switch();
        assert_eq!(EthernetSwitch::new(&mut snmp).port_name_by_mac("dc:a6:32:00:00:01").as_deref(), Some("ge-0/0/1"));
    }

    #[test]
    fn prefers_alias() {
        let mut snmp = switch();
        assert_eq!(EthernetSwitch::new(&mut snmp).port_name_by_mac("dc:a6:32:00:00:02").as_deref(), Some("jig 2"));
    }

    #[test]
    fn unknown_mac() {
        let mut snmp = switch();
        assert_eq!(EthernetSwitch::new(&mut snmp).port_name_by_mac("dc:a6:32:00:00:09"), None);
    }

    #[test]
    fn mac_on_multiple_vlans_is_ambiguous() {
        let mut snmp = switch();
        let mut vlan2 = vec![2u64];
        vlan2.extend([0xdc, 0xa6, 0x32, 0, 0, 1]);
        snmp.set(MACTABLE_MAC, &vlan2, SnmpValue::Bytes(vec![0xdc, 0xa6, 0x32, 0, 0, 1]));
        snmp.set(MACTABLE_PORT, &vlan2, SnmpValue::Integer(2));

        assert_eq!(EthernetSwitch::new(&mut snmp).port_name_by_mac("dc:a6:32:00:00:01").as_deref(), Some(MULTIPLE_PORTS));
    }

    #[test]
    fn full_table_caches_lookups() {
        let mut snmp = switch();
        snmp.fdb([0xdc, 0xa6, 0x32, 0, 0, 3], 1);

        let table = EthernetSwitch::new(&mut snmp).mac_to_port_name().unwrap();

        assert_eq!(table["dc:a6:32:00:00:01"], "ge-0/0/1");
        assert_eq!(table["dc:a6:32:00:00:02"], "jig 2");
        assert_eq!(table["dc:a6:32:00:00:03"], "ge-0/0/1");
        assert_eq!(snmp.gets, 2 + 3, "baseport and ifAlias/ifName lookups per port, cached across MACs");
    }

    #[test]
    fn empty_bridge_table_is_unsupported() {
        let mut snmp = FakeSnmp::default();
        assert!(EthernetSwitch::new(&mut snmp).mac_to_port_name().is_none());
    }
}
