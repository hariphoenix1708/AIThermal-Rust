use super::profile::StorageProfile;
use std::fs;
use std::path::Path;

#[allow(clippy::collapsible_if)]
pub fn probe_storage() -> StorageProfile {
    let mut profile = StorageProfile::default();

    let mut has_ufs = false;

    if let Ok(entries) = fs::read_dir("/sys/block") {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().to_string();

            // Skip loopback, ram, zram, and dm mapper devices
            if name.starts_with("loop")
                || name.starts_with("ram")
                || name.starts_with("zram")
                || name.starts_with("dm-")
            {
                continue;
            }

            profile.block_devices.push(name.clone());

            let path = entry.path();

            // Check UFS by device symlink or name
            if name.starts_with("sd") {
                if let Ok(target) = fs::read_link(path.join("device")) {
                    if target.to_string_lossy().contains("ufs") {
                        has_ufs = true;
                    }
                }
            }

            let queue_path = path.join("queue");

            if let Ok(content) = fs::read_to_string(queue_path.join("scheduler")) {
                profile
                    .io_schedulers
                    .insert(name.clone(), content.trim().to_string());

                // Parse available and current
                let mut avail = Vec::new();
                let mut current = String::new();

                for word in content.split_whitespace() {
                    if word.starts_with('[') && word.ends_with(']') {
                        let cur = word[1..word.len() - 1].to_string();
                        current = cur.clone();
                        avail.push(cur);
                    } else {
                        avail.push(word.to_string());
                    }
                }

                if !current.is_empty() {
                    profile.current_schedulers.insert(name.clone(), current);
                }
                profile.available_schedulers.insert(name.clone(), avail);
            }

            if let Ok(content) = fs::read_to_string(queue_path.join("read_ahead_kb")) {
                if let Ok(val) = content.trim().parse() {
                    profile.read_ahead_kb.insert(name.clone(), val);
                }
            }

            if let Ok(content) = fs::read_to_string(queue_path.join("nr_requests")) {
                if let Ok(val) = content.trim().parse() {
                    profile.nr_requests.insert(name.clone(), val);
                }
            }

            if let Ok(content) = fs::read_to_string(queue_path.join("rotational")) {
                if let Ok(val) = content.trim().parse::<u32>() {
                    profile.rotational.insert(name.clone(), val == 1);
                }
            }
        }
    }

    // Fallback UFS check
    if !has_ufs {
        has_ufs = Path::new("/sys/class/scsi_host/host0/device/ufs").exists()
            || Path::new("/sys/kernel/debug/ufshcd0").exists();
    }

    profile.has_ufs = has_ufs;

    profile
}
