use super::capability::CapabilityNode;
use super::profile::CpusetProfile;
use std::path::Path;

pub fn probe_cpuset() -> CpusetProfile {
    let mut profile = CpusetProfile::default();

    // v1 first (fastest path on HyperOS + older AOSP)
    for base in ["/dev/cpuset", "/sys/fs/cgroup/cpuset"] {
        if Path::new(base).join("tasks").exists()
            || Path::new(base).join("top-app").exists()
        {
            profile.root_path = base.to_string();
            profile.is_cgroup_v2 = false;
            profile.controller_ok = true;
            if Path::new(&format!("{}/top-app", base)).exists() {
                profile.top_app_path = Some(format!("{}/top-app", base));
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/top-app/cpus", base),
                    "cpuset_top_app",
                ));
            }
            if Path::new(&format!("{}/foreground", base)).exists() {
                profile.foreground_path = Some(format!("{}/foreground", base));
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/foreground/cpus", base),
                    "cpuset_foreground",
                ));
            }
            if Path::new(&format!("{}/background", base)).exists() {
                profile.background_path = Some(format!("{}/background", base));
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/background/cpus", base),
                    "cpuset_background",
                ));
            }
            if Path::new(&format!("{}/system-background", base)).exists() {
                profile.system_background_path = Some(format!("{}/system-background", base));
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/system-background/cpus", base),
                    "cpuset_sys_bg",
                ));
            }
            if Path::new(&format!("{}/restricted", base)).exists() {
                profile.restricted_path = Some(format!("{}/restricted", base));
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/restricted/cpus", base),
                    "cpuset_restricted",
                ));
            }
            return profile;
        }
    }

    // v2 unified hierarchy
    let v2_root = "/sys/fs/cgroup";
    if Path::new(&format!("{}/cgroup.controllers", v2_root)).exists() {
        let controllers = std::fs::read_to_string(
            format!("{}/cgroup.controllers", v2_root)
        ).unwrap_or_default();
        if controllers.split_whitespace().any(|c| c == "cpuset") {
            profile.root_path      = v2_root.to_string();
            profile.is_cgroup_v2   = true;
            profile.controller_ok  = true;

            profile.top_app_path          = Some(format!("{}/top-app",           v2_root));
            profile.foreground_path       = Some(format!("{}/foreground",        v2_root));
            profile.background_path       = Some(format!("{}/background",        v2_root));
            profile.system_background_path= Some(format!("{}/system-background", v2_root));
            profile.restricted_path       = Some(format!("{}/restricted",        v2_root));

            // push nodes
            profile.cpuset_nodes.push(CapabilityNode::new(
                &format!("{}/top-app/cpuset.cpus", v2_root),
                "cpuset_top_app",
            ));
            profile.cpuset_nodes.push(CapabilityNode::new(
                &format!("{}/foreground/cpuset.cpus", v2_root),
                "cpuset_foreground",
            ));
            profile.cpuset_nodes.push(CapabilityNode::new(
                &format!("{}/background/cpuset.cpus", v2_root),
                "cpuset_background",
            ));
            profile.cpuset_nodes.push(CapabilityNode::new(
                &format!("{}/system-background/cpuset.cpus", v2_root),
                "cpuset_sys_bg",
            ));
            profile.cpuset_nodes.push(CapabilityNode::new(
                &format!("{}/restricted/cpuset.cpus", v2_root),
                "cpuset_restricted",
            ));

            return profile;
        }
    }

    profile
}
