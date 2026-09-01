//! Phase 1 lightweight shadow MLP — no tract, 0.1ms on A520.
//! Reads JSON weights exported by python/train/train_mlp.py:
//! { "coefs": [ [8,16], [16,8], [8,1] ], "intercepts": [ [16], [8], [1] ] }
//! Input 8: composite/100, adj/100, trend/50, cpu/100, gpu/100, batt/100, gpu_load/100, cpu_pressure/100
//! Output 1: delta_x10 (±50 → ±5C)

use serde::Deserialize;
use std::sync::OnceLock;

#[derive(Debug, Deserialize)]
struct MlJson {
    coefs: Vec<Vec<Vec<f32>>>,      // 3 matrices
    intercepts: Vec<Vec<f32>>,       // 3 biases
    hidden_layer_sizes: Option<Vec<usize>>,
}

struct LoadedMl {
    coefs: Vec<Vec<Vec<f32>>>,
    intercepts: Vec<Vec<f32>>,
}

static MODEL: OnceLock<Option<LoadedMl>> = OnceLock::new();
static MODEL_PATH: OnceLock<String> = OnceLock::new();

fn load_once(path: &str) -> &'static Option<LoadedMl> {
    MODEL.get_or_init(|| {
        if path.is_empty() {
            tracing::debug!(target: "ml", "ml model path empty, shadow disabled");
            return None;
        }
        // Try primary path + siblings, then bundled fallback + state external
        let mut candidates = vec![
            path.to_string(),
            format!("{}.json", path),
            path.replace(".onnx", ".json"),
        ];
        // Bundled inside module (survives clean install) + state external (retrain override)
        candidates.push("/data/adb/modules/thermalai_rust/config/ml_model.json".to_string());
        candidates.push("/data/local/tmp/AIThermal/state/ml_model.onnx.json".to_string());
        candidates.push("/data/local/tmp/AIThermal/state/ml_model.json".to_string());
        for cand in candidates {
            if std::path::Path::new(&cand).exists() {
                match try_load(&cand) {
                    Ok(m) => {
                        tracing::info!(target: "ml", "ml shadow model loaded from '{}'", cand);
                        return Some(m);
                    }
                    Err(e) => {
                        tracing::warn!(target: "ml", "ml load failed '{}': {}", cand, e);
                    }
                }
            }
        }
        tracing::debug!(target: "ml", "ml model not found for '{}', shadow disabled (collecting only)", path);
        None
    })
}

fn try_load(path: &str) -> anyhow::Result<LoadedMl> {
    let data = std::fs::read_to_string(path)?;
    let j: MlJson = serde_json::from_str(&data)?;
    if j.coefs.len() != 3 || j.intercepts.len() != 3 {
        anyhow::bail!("expected 3 layers, got coefs {} intercepts {}", j.coefs.len(), j.intercepts.len());
    }
    Ok(LoadedMl { coefs: j.coefs, intercepts: j.intercepts })
}

fn relu(v: f32) -> f32 { if v > 0.0 { v } else { 0.0 } }

fn matvec_mul(input: &[f32], weights: &[Vec<f32>], bias: &[f32]) -> Vec<f32> {
    // weights: [in_dim][out_dim] as stored by sklearn (transposed)
    // sklearn coefs[0] shape (n_in, n_out) = (8,16): coefs[0][i][j] = w i->j
    let out_dim = bias.len();
    let mut out = vec![0.0f32; out_dim];
    for (i, row) in weights.iter().enumerate() {
        let inp = input[i];
        for (j, w) in row.iter().enumerate().take(out_dim) {
            out[j] += inp * *w;
        }
    }
    for (j, b) in bias.iter().enumerate() {
        out[j] += *b;
        out[j] = relu(out[j]);
    }
    out
}

fn forward(input: &[f32; 8], m: &LoadedMl) -> f32 {
    // layer0: 8 -> 16
    let h1 = matvec_mul(input, &m.coefs[0], &m.intercepts[0]);
    // layer1: 16 -> 8 (relu inside matvec)
    let h2 = matvec_mul(&h1, &m.coefs[1], &m.intercepts[1]);
    // layer2: 8 -> 1 no relu on output
    let w2 = &m.coefs[2];
    let b2 = &m.intercepts[2];
    let mut out = b2[0];
    for (i, row) in w2.iter().enumerate() {
        out += h2[i] * row[0];
    }
    out
}

/// Returns delta_x10 (±50) or None if no model / error. <0.1ms.
pub fn shadow_predict(
    model_path: &str,
    composite: i32,
    adj: i32,
    trend_score: i32,
    cpu_c: i32,
    gpu_c: i32,
    batt_c: i32,
    gpu_load: u32,
    cpu_pressure: f32,
) -> Option<i32> {
    let cached = MODEL_PATH.get_or_init(|| model_path.to_string());
    if cached != model_path {
        tracing::debug!(target: "ml", "ml path changed {} -> {} (restart to reload)", cached, model_path);
        return None;
    }
    let opt = load_once(model_path);
    let m = opt.as_ref()?;
    let input: [f32; 8] = [
        composite as f32 / 100.0,
        adj as f32 / 100.0,
        trend_score as f32 / 50.0,
        cpu_c as f32 / 100.0,
        gpu_c as f32 / 100.0,
        batt_c as f32 / 100.0,
        gpu_load as f32 / 100.0,
        (cpu_pressure / 100.0).clamp(0.0, 1.0),
    ];
    let delta_x10 = forward(&input, m).round() as i32;
    Some(delta_x10.clamp(-50, 50))
}

pub fn is_available(model_path: &str) -> bool {
    load_once(model_path).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_relu_forward_identity() {
        // smoke: empty model not loaded => None
        assert!(shadow_predict("/nonexistent", 40, 40, 0, 40, 40, 36, 10, 0.0).is_none());
    }
}
