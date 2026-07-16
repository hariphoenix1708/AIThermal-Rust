use anyhow::Result;
use std::env;
use std::path::Path;

use thermalai_daemon::hardware;

fn main() -> Result<()> {
    let state_dir = env::var("THERMALAI_STATE_DIR")
        .unwrap_or_else(|_| "/data/local/tmp/thermalai_state".to_string());

    if !Path::new(&state_dir).exists() {
        std::fs::create_dir_all(&state_dir)?;
    }

    println!("Probing hardware...");
    let profile = hardware::discovery::discover_force_rescan(&state_dir)?;

    let csv_path = Path::new(&state_dir).join("hardware_profile.csv");
    let mut csv_out = String::new();
    csv_out.push_str("Category,Key,Value\n");
    csv_out.push_str(&format!("Metadata,Device,{}\n", profile.device_identity));
    csv_out.push_str(&format!("Metadata,SoC,{}\n", profile.soc_info));
    csv_out.push_str(&format!(
        "Metadata,Kernel,{}\n",
        profile.metadata.kernel_version
    ));
    csv_out.push_str(&format!("GPU,Path,{}\n", profile.gpu_profile.path));
    csv_out.push_str(&format!("GPU,Is_KGSL,{}\n", profile.gpu_profile.is_kgsl));
    csv_out.push_str(&format!(
        "Thermal,Total_Zones,{}\n",
        profile.thermal_profile.all_zones.len()
    ));

    for (category, status) in &profile.compatibility_report {
        csv_out.push_str(&format!("Compatibility,{},{}\n", category, status));
    }
    std::fs::write(&csv_path, csv_out)?;

    println!("Done. See {} for results (TXT, JSON, CSV).", state_dir);
    Ok(())
}
