use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CapabilityNode {
    pub path: String,
    pub name: String,
    pub readable: bool,
    pub writable: bool,
    pub valid: bool,
    pub supported_values: Vec<String>,
}

impl CapabilityNode {
    pub fn new(path: &str, name: &str) -> Self {
        let p = std::path::Path::new(path);
        let exists = p.exists();
        let readable = exists && std::fs::File::open(p).is_ok();
        let writable = exists && std::fs::OpenOptions::new().write(true).open(p).is_ok();

        let mut supported_values = Vec::new();
        let available_path = format!("{}_available", path);
        if std::path::Path::new(&available_path).exists() {
            if let Ok(content) = std::fs::read_to_string(&available_path) {
                supported_values = content.split_whitespace().map(|s| s.to_string()).collect();
            }
        } else if std::path::Path::new(&path.replace("current", "available")).exists() {
            if let Ok(content) = std::fs::read_to_string(&path.replace("current", "available")) {
                supported_values = content.split_whitespace().map(|s| s.to_string()).collect();
            }
        }

        Self {
            path: path.to_string(),
            name: name.to_string(),
            readable,
            writable,
            valid: exists && (readable || writable),
            supported_values,
        }
    }
}
