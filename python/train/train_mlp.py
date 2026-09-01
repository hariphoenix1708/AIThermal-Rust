#!/usr/bin/env python3
"""
Thermal Δ MLP trainer — matches rust/src/prediction/ml.rs shadow.

Input  [8]: composite/100, adj/100, trend/50, cpu/100, gpu/100, batt/100, gpu_load/100, cpu_pressure/100
Output [1]: delta_x10 = (temp+10s - adj) *10  clamped ±50 (→ ±5C)

Dataset: ml_features.jsonl from daemon (v1). Each row has composite_c etc. + policy.
We derive label by looking 5 ticks ahead (≈10s at 2s poll) — script does temporal join.

Usage:
  python3 train_mlp.py --data /path/to/ml_features.jsonl --out thermal_mlp.onnx
  # then quantize:
  python3 -m onnxruntime.quantization.quantize --input thermal_mlp.onnx --output thermal_mlp.int8.onnx

Requires: torch, onnx, sklearn (fallback), numpy
"""
import argparse, json, pathlib
import numpy as np

FEATURES = ["composite_c","adj_c","trend_score","cpu_c","gpu_c","batt_c","gpu_load","cpu_pressure"]

def load_rows(path, horizon=5):
    import glob, os
    # Support single file, directory, or pattern; also load incrementing .1..5 siblings for training
    candidates = []
    is_ml = "ml_features" in os.path.basename(path)
    if os.path.isdir(path):
        # directory: load all ml_features.jsonl* inside
        candidates = glob.glob(os.path.join(path, "ml_features.jsonl*"))
    elif "*" in path or "?" in path:
        candidates = glob.glob(path)
    else:
        candidates.append(path)
        if is_ml:
            # Also load siblings .1..5 if base file given (for incrementing rotation)
            base = path
            for i in range(1, 6):
                cand = f"{base}.{i}"
                if os.path.exists(cand) and cand not in candidates:
                    candidates.append(cand)
            # Also glob for any ml_features.jsonl* in same dir (covers .1..5 already, but deduped)
            dirn = os.path.dirname(base) or "."
            extra = glob.glob(os.path.join(dirn, "ml_features.jsonl*"))
            for e in extra:
                if e not in candidates and os.path.basename(e) != os.path.basename(path):
                    # Only add if not already in candidates and not the base itself
                    # Avoid double-counting when passing a combined file that is not ml_features
                    candidates.append(e)
    rows = []
    for cand in sorted(set(candidates)):
        try:
            with open(cand) as f:
                for line in f:
                    try:
                        j = json.loads(line)
                        if j.get("v") != 1 or "event" in j: continue
                        rows.append(j)
                    except: continue
        except FileNotFoundError:
            continue
    # sort by ts, join horizon ahead for label
    rows.sort(key=lambda r: r["ts"])
    data = []
    for i in range(len(rows)-horizon):
        cur = rows[i]; fut = rows[i+horizon]
        # label: adj delta 10s later
        delta = fut["adj_c"] - cur["adj_c"]
        # filter crazy jumps (>10C in 10s is sensor glitch)
        if abs(delta) > 10: continue
        x = np.array([
            cur["composite_c"]/100.0,
            cur["adj_c"]/100.0,
            cur["trend_score"]/50.0,
            cur["cpu_c"]/100.0,
            cur["gpu_c"]/100.0,
            cur["batt_c"]/100.0,
            cur["gpu_load"]/100.0,
            float(cur.get("cpu_pressure",0))/100.0,
        ], dtype=np.float32)
        y = np.array([delta*10], dtype=np.float32)  # x10 for tract
        data.append((x,y))
    return data

def train_sklearn(data, out):
    try:
        from sklearn.neural_network import MLPRegressor
        from sklearn.model_selection import train_test_split
        X = np.stack([x for x,y in data]); Y = np.stack([y for x,y in data]).ravel()
        Xtr, Xte, Ytr, Yte = train_test_split(X,Y,test_size=0.2, random_state=0)
        mlp = MLPRegressor(hidden_layer_sizes=(16,8), activation="relu", max_iter=800, random_state=0, verbose=True)
        mlp.fit(Xtr, Ytr)
        from sklearn.metrics import mean_absolute_error
        pred = mlp.predict(Xte)
        print(f"MAE x10: {mean_absolute_error(Yte,pred):.2f}  MAE C: {mean_absolute_error(Yte,pred)/10:.2f}")
        # Export via onnx (skl2onnx) + ALWAYS write JSON for lightweight Rust (no tract)
        import json as js
        # JSON for manual Rust MLP (fast, no tract)
        js.dump({
            "coefs": [c.tolist() for c in mlp.coefs_],
            "intercepts": [b.tolist() for b in mlp.intercepts_],
            "hidden_layer_sizes": list(mlp.hidden_layer_sizes),
        }, open(out+".json","w"))
        print(f"Wrote {out}.json (manual)")
        try:
            from skl2onnx import convert_sklearn
            from skl2onnx.common.data_types import FloatTensorType
            onx = convert_sklearn(mlp, initial_types=[("input", FloatTensorType([None, 8]))])
            pathlib.Path(out).write_bytes(onx.SerializeToString())
            print(f"Wrote {out} via skl2onnx")
        except Exception as e:
            print(f"skl2onnx onnx not written: {e}")
        return
    except ImportError as e:
        print("sklearn not available", e)

def train_torch(data, out):
    try:
        import torch, torch.nn as nn
        X = np.stack([x for x,y in data]); Y = np.stack([y for x,y in data])
        Xt = torch.from_numpy(X); Yt = torch.from_numpy(Y)
        model = nn.Sequential(nn.Linear(8,16), nn.ReLU(), nn.Linear(16,8), nn.ReLU(), nn.Linear(8,1))
        opt = torch.optim.Adam(model.parameters(), lr=1e-3)
        loss_fn = nn.L1Loss()
        for epoch in range(400):
            opt.zero_grad(); pred = model(Xt); loss = loss_fn(pred, Yt); loss.backward(); opt.step()
            if epoch%80==0: print(f"epoch {epoch} loss {loss.item():.3f}")
        # export ONNX
        torch.onnx.export(model, Xt[:1], out, input_names=["input"], output_names=["output"], dynamic_axes={"input":{0:"batch"},"output":{0:"batch"}})
        print(f"Wrote {out} via torch")
    except ImportError as e:
        print("torch not available, falling back to sklearn", e)
        train_sklearn(data, out)

if __name__ == "__main__":
    ap = argparse.ArgumentParser()
    ap.add_argument("--data", required=True)
    ap.add_argument("--out", default="thermal_mlp.onnx")
    ap.add_argument("--horizon", type=int, default=5)
    args = ap.parse_args()
    data = load_rows(args.data, args.horizon)
    print(f"loaded {len(data)} samples from {args.data}")
    if len(data) < 200:
        print("Need ≥200 samples (≥30min). Collect more via ml_features.jsonl.")
    else:
        # try torch first, else sklearn
        try:
            import torch
            train_torch(data, args.out)
        except ImportError:
            train_sklearn(data, args.out)
