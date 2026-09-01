# Train

Dataset: `adb pull /data/local/tmp/AIThermal/state/ml_features.jsonl` (v1, 2s/tick + 2MB ring).

```bash
pip install torch onnx sklearn skl2onnx numpy
python3 train_mlp.py --data ml_features.jsonl --out thermal_mlp.onnx
# quantize to int8 (keeps <1% MAE loss for this head):
python3 -c "import onnxruntime.quantization; print('quantize via onnxruntime tooling')"
# deploy
adb push thermal_mlp.onnx /data/local/tmp/AIThermal/state/ml_model.onnx
adb shell su -c 'echo "ml_shadow_enabled = true\nml_model_path = \"/data/local/tmp/AIThermal/state/ml_model.onnx\"" >> /data/adb/modules/thermalai_rust/config/profiles.conf; thermalair restart'
adb shell su -c 'cat /data/local/tmp/AIThermal/thermalai_verbose.log | grep "ml shadow" | tail'
```

Expected shadow MAE `0.4-0.6C` vs linear `2C` on SM8635 peribot. Replace linear only after shadow `err` stable 1 week.
