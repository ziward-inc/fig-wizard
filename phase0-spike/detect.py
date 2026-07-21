#!/usr/bin/env python3
"""
Phase-0 spike: run PP-DocLayoutV3 ONNX (community export from
alex-dinh/PP-DocLayoutV3-ONNX on HuggingFace) on rendered arXiv page images,
draw the resulting boxes + labels, and save annotated images for review.

Model IO (verified via onnxruntime introspection):
  inputs:  im_shape (N,2) float32, image (N,3,800,800) float32, scale_factor (N,2) float32
  outputs: fetch_name_0 (N*maxdet, 7) float32 = [label_idx, score, xmin, ymin, xmax, ymax, read_order]
           fetch_name_1 (N,) int32  = number of valid detections per image (NMS'd count)
           fetch_name_2 (N,200,200) int32 = unused auxiliary mask output (ignored here)

This is a Paddle Detection "DETR"-style export: NMS and box decoding are baked
into the graph already (this is why the postprocess config has no
NMS/threshold section - only Resize/Normalize/Permute pre-processing). We
therefore do NOT need to run our own NMS; we only apply a score threshold.
"""
import json
import os

import cv2
import numpy as np
import onnxruntime as ort
from PIL import Image, ImageDraw, ImageFont

MODEL_PATH = "models/PP-DocLayoutV3.onnx"
PAGES_DIR = "pages"
OUT_DIR = "output"
TARGET_SIZE = (800, 800)  # (H, W), from config.json Preprocess.Resize.target_size
SCORE_THRESH = 0.4

with open("hf_meta/config.json") as f:
    CFG = json.load(f)
LABELS = CFG["label_list"]

# Deterministic color per label (BGR-ish doesn't matter, we draw with PIL/RGB)
def color_for(idx):
    rng = np.random.default_rng(idx * 7919 + 13)
    c = rng.integers(40, 230, size=3)
    return (int(c[0]), int(c[1]), int(c[2]))

LABEL_COLORS = {name: color_for(i) for i, name in enumerate(LABELS)}


def preprocess(image_bgr, target_input_size=TARGET_SIZE):
    """
    Preprocessing recipe determined empirically (see preprocess_ab_test.py).

    Neither of the two documented recipes was fully correct:
      - The HF README/example script applies ImageNet mean/std normalization
        after /255 scaling. This *works* (valid boxes) but under-confidences
        detections relative to the alternative below.
      - config.json's Preprocess block literally specifies
        NormalizeImage(mean=0, std=1, norm_type="none"), which we interpreted
        as "skip the /255 scaling too" (raw 0-255 pixel values). That recipe
        produced ZERO detections above threshold - clearly wrong.

    The empirically-best recipe (highest, most consistent confidence scores
    across multiple test pages, near-identical box locations to the README
    recipe but tighter) is: BGR->RGB, scale to [0,1] via /255, and *no*
    ImageNet mean/std shift. This is what we use here.
    """
    orig_h, orig_w = image_bgr.shape[:2]
    target_h, target_w = target_input_size
    scale_h = target_h / orig_h
    scale_w = target_w / orig_w
    resized = cv2.resize(image_bgr, (target_w, target_h), interpolation=cv2.INTER_LINEAR)
    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    blob = rgb.astype(np.float32) / 255.0
    blob = blob.transpose(2, 0, 1)  # CHW
    return blob, scale_h, scale_w


def run_one(session, input_names, output_names, image_path):
    img = cv2.imread(image_path)
    if img is None:
        raise FileNotFoundError(image_path)
    blob, scale_h, scale_w = preprocess(img)
    im_shape = np.array([[TARGET_SIZE[0], TARGET_SIZE[1]]], dtype=np.float32)  # (1,2)
    image_in = blob[np.newaxis, ...].astype(np.float32)  # (1,3,800,800)
    scale_factor = np.array([[scale_h, scale_w]], dtype=np.float32)  # (1,2)

    feed = {
        input_names[0]: im_shape,
        input_names[1]: image_in,
        input_names[2]: scale_factor,
    }
    outputs = session.run(output_names, feed)
    dets = outputs[0]  # (maxdet, 7): label, score, x0, y0, x1, y1, read_order
    n_valid = int(outputs[1][0]) if len(outputs) > 1 else dets.shape[0]
    dets = dets[:n_valid]
    return img, dets


def draw_and_save(img_bgr, dets, out_path, score_thresh=SCORE_THRESH):
    img_rgb = cv2.cvtColor(img_bgr, cv2.COLOR_BGR2RGB)
    pil_img = Image.fromarray(img_rgb)
    draw = ImageDraw.Draw(pil_img)
    try:
        font = ImageFont.truetype("/System/Library/Fonts/Supplemental/Arial.ttf", 22)
    except Exception:
        font = ImageFont.load_default()

    kept = []
    for det in dets:
        label_idx, score = int(det[0]), float(det[1])
        x0, y0, x1, y1 = (float(v) for v in det[2:6])
        if score < score_thresh:
            continue
        if label_idx < 0 or label_idx >= len(LABELS):
            name = f"cls_{label_idx}"
            color = (255, 0, 0)
        else:
            name = LABELS[label_idx]
            color = LABEL_COLORS[name]
        kept.append((name, score, x0, y0, x1, y1))
        draw.rectangle([x0, y0, x1, y1], outline=color, width=3)
        text = f"{name} {score:.2f}"
        tb = draw.textbbox((x0, y0), text, font=font)
        draw.rectangle([tb[0], tb[1] - 2, tb[2] + 4, tb[3] + 2], fill=color)
        draw.text((x0 + 2, y0 - 2), text, fill=(0, 0, 0), font=font)

    pil_img.save(out_path)
    return kept


def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    session = ort.InferenceSession(MODEL_PATH, providers=["CPUExecutionProvider"])
    input_names = [i.name for i in session.get_inputs()]
    output_names = [o.name for o in session.get_outputs()]
    print("input_names:", input_names)
    print("output_names:", output_names)

    page_files = sorted(f for f in os.listdir(PAGES_DIR) if f.endswith(".png"))
    summary = {}
    for fname in page_files:
        in_path = os.path.join(PAGES_DIR, fname)
        out_path = os.path.join(OUT_DIR, fname.replace(".png", "_annotated.png"))
        img, dets = run_one(session, input_names, output_names, in_path)
        kept = draw_and_save(img, dets, out_path)
        summary[fname] = kept
        print(f"\n=== {fname} -> {out_path} ===")
        for name, score, x0, y0, x1, y1 in kept:
            print(f"  {name:20s} {score:.3f}  [{x0:.0f},{y0:.0f},{x1:.0f},{y1:.0f}]")

    with open(os.path.join(OUT_DIR, "summary.json"), "w") as f:
        json.dump(
            {
                fname: [
                    {"label": n, "score": s, "box": [x0, y0, x1, y1]}
                    for n, s, x0, y0, x1, y1 in kept
                ]
                for fname, kept in summary.items()
            },
            f,
            indent=2,
        )


if __name__ == "__main__":
    main()
