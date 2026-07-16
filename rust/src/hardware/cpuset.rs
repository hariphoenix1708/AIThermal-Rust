use super::capability::CapabilityNode;
use super::profile::CpusetProfile;
use std::path::Path;

pub fn probe_cpuset() -> CpusetProfile {
    let mut profile = CpusetProfile::default();

    let bases = ["/dev/cpuset", "/sys/fs/cgroup/cpuset"];
    for base in bases {
        if Path::new(&format!("{}/background", base)).exists() {
            profile.root_path = base.to_string();
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
                profile.cpuset_nodes.push(CapabilityNode::new(
                    &format!("{}/restricted/cpus", base),
                    "cpuset_restricted",
                ));
            }
            break;
        }
    }

    profile
}
