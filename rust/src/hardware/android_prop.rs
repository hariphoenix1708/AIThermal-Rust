use std::fs;
use std::process::Command;

pub fn get_property(key: &str, default: &str) -> String {
    // Attempt 1: getprop binary
    #[allow(clippy::collapsible_if)]
    if let Ok(output) = Command::new("getprop").arg(key).output() {
        if output.status.success() {
            let val = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !val.is_empty() {
                return val;
            }
        }
    }

    // Attempt 2: fallback to reading /system/build.prop or /vendor/build.prop if getprop fails/missing (e.g. testing)
    let prop_files = [
        "/default.prop",
        "/system/build.prop",
        "/vendor/build.prop",
        "/product/build.prop",
    ];

    for file in &prop_files {
        if let Ok(content) = fs::read_to_string(file) {
            for line in content.lines() {
                #[allow(clippy::collapsible_if)]
                if line.starts_with(key) {
                    if let Some(idx) = line.find('=') {
                        return line[idx + 1..].trim().to_string();
                    }
                }
            }
        }
    }

    default.to_string()
}

pub fn get_any_property(keys: &[&str], default: &str) -> String {
    for key in keys {
        let val = get_property(key, "");
        if !val.is_empty() {
            return val;
        }
    }
    default.to_string()
}
