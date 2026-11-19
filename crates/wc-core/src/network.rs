//! Fieldbus networks (PROFINET, EtherCAT, ...): which devices are on a bus, their addresses,
//! and the cables between their ports. The network list report lists them, linked to the pages
//! the devices are drawn on.
//!
//! They are kept in `project.toml` as `[[network]]` tables, each with its `[[network.node]]`s
//! and `[[network.link]]`s:
//!
//! ```toml
//! [[network]]
//! name = "PN1"
//! kind = "profinet"
//! description = "Line 1 field devices"
//!
//! [[network.node]]
//! device = "-A1"
//! address = "192.168.0.1"
//! station = "plc-line1"
//!
//! [[network.node]]
//! device = "-K10"
//! address = "192.168.0.10"
//!
//! [[network.link]]
//! from = "-A1:X1P1"
//! to = "-K10:X1P1"
//! cable = "-W20"
//! ```
//!
//! A node is a device tag with its bus address (an IP address on PROFINET, a station address
//! on EtherCAT) and, for PROFINET, its station (device) name. A link joins two ports, written
//! `<device tag>:<port>`; the port can be left out (`-K10`). `kind` is free text, with
//! `profinet` and `ethercat` known by name. Unknown keys are kept when the project is saved.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::DeviceTag;

/// One fieldbus network.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Network {
    pub name: String,
    /// The bus type: `profinet`, `ethercat` or any other name.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub kind: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub description: String,
    #[serde(default, rename = "node", skip_serializing_if = "Vec::is_empty")]
    pub nodes: Vec<Node>,
    #[serde(default, rename = "link", skip_serializing_if = "Vec::is_empty")]
    pub links: Vec<Link>,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// A device on a network.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Node {
    /// The device's tag, such as `-K10`.
    pub device: String,
    /// Its bus address: an IP address, a station number.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub address: String,
    /// The PROFINET station (device) name, or another name the bus knows it by.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub station: String,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// A connection between two ports of a network, usually a cable.
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
pub struct Link {
    /// `<device tag>:<port>`, or just the tag.
    pub from: String,
    pub to: String,
    /// The cable's device tag, such as `-W20`.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub cable: String,
    #[serde(flatten, default, skip_serializing_if = "BTreeMap::is_empty")]
    pub unknown: BTreeMap<String, toml::Value>,
}

/// One end of a [`Link`]: a device and, when given, its port.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Port {
    pub device: DeviceTag,
    /// Empty when the link does not name a port.
    pub port: String,
}

impl Port {
    /// Read `<device tag>:<port>` or a bare tag.
    pub fn parse(s: &str) -> Result<Port, String> {
        let s = s.trim();
        let (tag, port) = match s.rsplit_once(':') {
            Some((tag, port)) => (tag.trim(), port.trim()),
            None => (s, ""),
        };
        let device = tag.parse::<DeviceTag>().map_err(|e| format!("{s:?}: {e}"))?;
        Ok(Port {
            device,
            port: port.to_owned(),
        })
    }
}

impl std::fmt::Display for Port {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        if self.port.is_empty() {
            write!(f, "{}", self.device)
        } else {
            write!(f, "{}:{}", self.device, self.port)
        }
    }
}

impl Network {
    /// The bus type as shown: `PROFINET`, `EtherCAT`, or the text as written.
    pub fn kind_label(&self) -> String {
        match self.kind.trim().to_ascii_lowercase().as_str() {
            "profinet" | "pn" => "PROFINET".into(),
            "ethercat" | "ecat" => "EtherCAT".into(),
            _ => self.kind.trim().to_owned(),
        }
    }

    /// The node of `device`, if it is on this network.
    pub fn node(&self, device: &DeviceTag) -> Option<&Node> {
        self.nodes
            .iter()
            .find(|n| n.device.parse::<DeviceTag>().ok().as_ref() == Some(device))
    }

    /// The links of this network that could be read, each with its two ends.
    pub fn ports(&self) -> Vec<(Port, Port, &Link)> {
        self.links
            .iter()
            .filter_map(|l| Some((Port::parse(&l.from).ok()?, Port::parse(&l.to).ok()?, l)))
            .collect()
    }

    /// What is wrong with this network: a node or link end that is not a device tag, a device
    /// listed twice, two nodes with one address, a link end on a device that is not a node of
    /// the network, a link from a port to itself, and a port used by two links.
    pub fn warnings(&self) -> Vec<String> {
        let name = &self.name;
        let mut out = Vec::new();
        if name.trim().is_empty() {
            out.push("a network has no name".to_owned());
        }
        let mut devices = BTreeSet::new();
        let mut addresses: BTreeMap<&str, &str> = BTreeMap::new();
        for n in &self.nodes {
            match n.device.parse::<DeviceTag>() {
                Ok(tag) => {
                    if !devices.insert(tag.clone()) {
                        out.push(format!("network {name}: {tag} is listed twice"));
                    }
                }
                Err(e) => out.push(format!("network {name}: node {:?}: {e}", n.device)),
            }
            let a = n.address.trim();
            if !a.is_empty()
                && let Some(other) = addresses.insert(a, n.device.as_str())
            {
                out.push(format!(
                    "network {name}: {} and {} have the same address {a}",
                    other, n.device
                ));
            }
        }
        let mut used: BTreeMap<Port, usize> = BTreeMap::new();
        for (i, l) in self.links.iter().enumerate() {
            let ends = [Port::parse(&l.from), Port::parse(&l.to)];
            for end in &ends {
                match end {
                    Ok(p) => {
                        if !devices.contains(&p.device) {
                            out.push(format!(
                                "network {name}: link {} is on {}, which is not a node",
                                i + 1,
                                p.device
                            ));
                        }
                        if !p.port.is_empty() {
                            *used.entry(p.clone()).or_default() += 1;
                        }
                    }
                    Err(e) => out.push(format!("network {name}: link {}: {e}", i + 1)),
                }
            }
            if let [Ok(a), Ok(b)] = &ends
                && a == b
            {
                out.push(format!("network {name}: link {} joins {a} to itself", i + 1));
            }
        }
        for (port, n) in used {
            if n > 1 {
                out.push(format!("network {name}: port {port} has {n} links"));
            }
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    type R = std::result::Result<(), Box<dyn std::error::Error>>;

    const TOML: &str = r#"
name = "PN1"
kind = "ProfiNet"
colour = "green"

[[node]]
device = "-A1"
address = "192.168.0.1"
station = "plc"

[[node]]
device = "-K10"
address = "192.168.0.10"

[[link]]
from = "-A1:X1P1"
to = "-K10:X1P1"
cable = "-W20"
"#;

    #[test]
    fn reads_and_writes_a_network_with_its_unknown_keys() -> R {
        let n: Network = toml::from_str(TOML)?;
        assert_eq!(n.kind_label(), "PROFINET");
        assert_eq!(n.nodes.len(), 2);
        assert_eq!(
            n.node(&"-K10".parse()?).map(|n| n.address.as_str()),
            Some("192.168.0.10")
        );
        let ports = n.ports();
        assert_eq!(ports.len(), 1);
        assert_eq!(ports[0].0.to_string(), "-A1:X1P1");
        assert_eq!(ports[0].1.port, "X1P1");
        assert!(n.warnings().is_empty(), "{:?}", n.warnings());
        let again: Network = toml::from_str(&toml::to_string(&n)?)?;
        assert_eq!(again, n);
        assert!(again.unknown.contains_key("colour"));
        Ok(())
    }

    #[test]
    fn a_port_can_leave_out_its_port_name() -> R {
        let p = Port::parse(" =A1+L2-K3 ")?;
        assert_eq!(p.port, "");
        assert_eq!(p.to_string(), "=A1+L2-K3");
        assert!(Port::parse("K 3:P1").is_err());
        Ok(())
    }

    #[test]
    fn warns_about_duplicates_strangers_and_ports_used_twice() -> R {
        let mut n: Network = toml::from_str(TOML)?;
        n.nodes.push(Node {
            device: "-K11".into(),
            address: "192.168.0.10".into(),
            ..Default::default()
        });
        n.nodes.push(Node {
            device: "-K10".into(),
            ..Default::default()
        });
        for (from, to) in [("-A1:X1P1", "-K11:P1"), ("-K11:P2", "-K99:P1"), ("-K11:P2", "-K11:P2")] {
            n.links.push(Link {
                from: from.into(),
                to: to.into(),
                ..Default::default()
            });
        }
        assert_eq!(
            n.warnings(),
            [
                "network PN1: -K10 and -K11 have the same address 192.168.0.10",
                "network PN1: -K10 is listed twice",
                "network PN1: link 3 is on -K99, which is not a node",
                "network PN1: link 4 joins -K11:P2 to itself",
                "network PN1: port -A1:X1P1 has 2 links",
                "network PN1: port -K11:P2 has 3 links",
            ]
        );
        Ok(())
    }
}
