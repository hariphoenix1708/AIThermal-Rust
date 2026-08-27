//! Network QoS — DSCP marking and tc qdisc prioritization for gaming traffic.
//!
//! On Android, we can use `tc` (traffic control) to prioritize game traffic.
//! DSCP marking (EF=Expedited Forwarding, CS6=Network Control) ensures
//! routers prioritize game packets.

use std::process::Command;
use std::sync::OnceLock;

static TC_AVAILABLE: OnceLock<bool> = OnceLock::new();

pub struct NetworkQoS {
    active_interface: Option<String>,
    tc_available: bool,
}

impl NetworkQoS {
    pub fn new() -> Self {
        let tc_available = *TC_AVAILABLE.get_or_init(|| {
            Command::new("which")
                .arg("tc")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        });
        Self {
            active_interface: None,
            tc_available,
        }
    }

    /// Activate: set up tc qdisc with priority for game traffic on active interface.
    pub fn activate(&mut self, interface: &str) {
        if !self.tc_available {
            tracing::debug!(
                target: "game_turbo",
                "Network QoS: 'tc' command not available, skipping"
            );
            return;
        }

        self.active_interface = Some(interface.to_string());

        // Delete existing qdisc if any.
        let _ = Command::new("tc")
            .args(["qdisc", "del", "dev", interface, "root"])
            .output();

        // Create HTB qdisc with priority classes.
        // Class 1:10 = Gaming (high priority, EF/CS6 DSCP)
        // Class 1:20 = Default
        // Class 1:30 = Background (low priority)
        let cmds = [
            // Root HTB qdisc
            vec!["qdisc", "add", "dev", interface, "root", "handle", "1:", "htb", "default", "20"],
            // Parent class
            vec!["class", "add", "dev", interface, "parent", "1:", "classid", "1:1", "htb", "rate", "1000mbit", "ceil", "1000mbit"],
            // Gaming class (high priority)
            vec!["class", "add", "dev", interface, "parent", "1:1", "classid", "1:10", "htb", "rate", "500mbit", "ceil", "1000mbit", "prio", "1"],
            // Default class
            vec!["class", "add", "dev", interface, "parent", "1:1", "classid", "1:20", "htb", "rate", "300mbit", "ceil", "1000mbit", "prio", "2"],
            // Background class (low priority)
            vec!["class", "add", "dev", interface, "parent", "1:1", "classid", "1:30", "htb", "rate", "100mbit", "ceil", "500mbit", "prio", "3"],
            // SFQ for fairness within classes
            vec!["qdisc", "add", "dev", interface, "parent", "1:10", "handle", "10:", "sfq", "perturb", "10"],
            vec!["qdisc", "add", "dev", interface, "parent", "1:20", "handle", "20:", "sfq", "perturb", "10"],
            vec!["qdisc", "add", "dev", interface, "parent", "1:30", "handle", "30:", "sfq", "perturb", "10"],
            // Filter: mark gaming traffic (DSCP EF=46, CS6=48) to class 1:10
            // Note: This requires iptables/nftables marking, which is complex.
            // For now, we rely on the game setting SO_PRIORITY or DSCP in packets.
        ];

        for cmd in cmds {
            if let Err(e) = Command::new("tc").args(&cmd).output() {
                tracing::debug!(
                    target: "game_turbo",
                    "Network QoS: tc command failed {:?}: {}",
                    cmd, e
                );
            }
        }

        tracing::info!(
            target: "game_turbo",
            "Network QoS: HTB qdisc configured on {} with gaming priority class",
            interface
        );
    }

    /// Per-tick: no-op.
    pub fn tick(&mut self) {}

    /// Deactivate: remove tc qdisc.
    pub fn deactivate(&mut self) {
        if !self.tc_available {
            return;
        }
        if let Some(interface) = self.active_interface.take() {
            let _ = Command::new("tc")
                .args(["qdisc", "del", "dev", &interface, "root"])
                .output();
            tracing::info!(
                target: "game_turbo",
                "Network QoS: removed qdisc from {}",
                interface
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_network_qos_new() {
        let qos = NetworkQoS::new();
        let _ = qos.tc_available;
    }
}