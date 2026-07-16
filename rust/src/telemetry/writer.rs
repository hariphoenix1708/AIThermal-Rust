use crate::runtime_context::RuntimeContext;
use serde_json::Value;
use std::fs;
use std::path::Path;
use tracing::error;

pub fn write_telemetry(ctx: &RuntimeContext, telemetry: &Value) {
    let state_path = Path::new(&ctx.state_dir).join("thermalai_state.json");
    let temp_path = Path::new(&ctx.state_dir).join("thermalai_state.json.tmp");

    if let Ok(json) = serde_json::to_string_pretty(telemetry) {
        if let Err(e) = fs::write(&temp_path, json) {
            error!("Failed to write state tmp: {}", e);
        } else if let Err(e) = fs::rename(&temp_path, &state_path) {
            error!("Failed to commit state: {}", e);
        }
    }
}
