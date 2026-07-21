#!/usr/bin/env python3
"""
A/B test of preprocessing recipes for PP-DocLayoutV3.onnx.

Two candidate recipes disagree:
  (A) README / batched_inference_example.py: BGR->RGB, /255, ImageNet mean/std normalize
  (B) config.json Preprocess block literally: Resize -> NormalizeImage(mean=0,std=1,norm_type="none") -> Permute
      i.e. no mean/std shift; "none" norm_type in PaddleDetection means the /255 scaling
      is also skipped, so raw 0-255 float32 pixel values are passed through.

We run both on the same page and compare max confidence / box count / class agreement
to determine which recipe the exported graph actually expects.
"""
import json

import cv2
import numpy as np
import onnxruntime as ort

MODEL_PATH = "models/PP-DocLayoutV3.onnx"
TEST_IMAGE = "pages/ppo_p05_algorithm.png"
TARGET_SIZE = (800, 800)

with open("hf_meta/config.json") as f:
    CFG = json.load(f)
LABELS = CFG["label_list"]


def resize_common(image_bgr):
    orig_h, orig_w = image_bgr.shape[:2]
    target_h, target_w = TARGET_SIZE
    scale_h = target_h / orig_h
    scale_w = target_w / orig_w
    resized = cv2.resize(image_bgr, (target_w, target_h), interpolation=cv2.INTER_LINEAR)
    return resized, scale_h, scale_w


def recipe_A_imagenet(image_bgr):
    resized, scale_h, scale_w = resize_common(image_bgr)
    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    blob = rgb.astype(np.float32) / 255.0
    mean = np.array([0.485, 0.456, 0.406], dtype=np.float32)
    std = np.array([0.229, 0.224, 0.225], dtype=np.float32)
    blob = (blob - mean) / std
    blob = blob.transpose(2, 0, 1)
    return blob, scale_h, scale_w


def recipe_B_raw_rgb(image_bgr):
    # config.json literally: Resize -> NormalizeImage(mean=0,std=1,norm_type=none) -> Permute
    # norm_type "none" in PaddleDetection skips the 1/255 scale too -> raw 0-255 float32.
    resized, scale_h, scale_w = resize_common(image_bgr)
    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    blob = rgb.astype(np.float32)  # raw 0-255, RGB order
    blob = blob.transpose(2, 0, 1)
    return blob, scale_h, scale_w


def recipe_C_raw_bgr(image_bgr):
    # Same as B but skip the BGR->RGB conversion (config lists no color-conversion op at all)
    resized, scale_h, scale_w = resize_common(image_bgr)
    blob = resized.astype(np.float32)  # raw 0-255, BGR order (as cv2.imread gives it)
    blob = blob.transpose(2, 0, 1)
    return blob, scale_h, scale_w


def recipe_D_scaled_rgb_no_meanstd(image_bgr):
    # /255 but no ImageNet mean/std shift
    resized, scale_h, scale_w = resize_common(image_bgr)
    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    blob = rgb.astype(np.float32) / 255.0
    blob = blob.transpose(2, 0, 1)
    return blob, scale_h, scale_w


def run(session, input_names, output_names, blob, scale_h, scale_w):
    im_shape = np.array([[TARGET_SIZE[0], TARGET_SIZE[1]]], dtype=np.float32)
    image_in = blob[np.newaxis, ...].astype(np.float32)
    scale_factor = np.array([[scale_h, scale_w]], dtype=np.float32)
    feed = {input_names[0]: im_shape, input_names[1]: image_in, input_names[2]: scale_factor}
    outputs = session.run(output_names, feed)
    dets = outputs[0]
    n_valid = int(outputs[1][0])
    return dets[:n_valid]


def summarize(name, dets, thresh=0.3):
    kept = dets[dets[:, 1] > thresh]
    n = len(kept)
    if n == 0:
        print(f"{name:30s} n_boxes={0:3d}  max_score=  n/a")
        return
    max_score = kept[:, 1].max()
    mean_score = kept[:, 1].mean()
    labels = sorted(set(LABELS[int(l)] if 0 <= int(l) < len(LABELS) else f"cls{int(l)}" for l in kept[:, 0]))
    print(f"{name:30s} n_boxes={n:3d}  max_score={max_score:.3f}  mean_score={mean_score:.3f}")
    print(f"  labels seen: {labels}")


def main():
    session = ort.InferenceSession(MODEL_PATH, providers=["CPUExecutionProvider"])
    input_names = [i.name for i in session.get_inputs()]
    output_names = [o.name for o in session.get_outputs()]

    img = cv2.imread(TEST_IMAGE)

    for name, fn in [
        ("A: RGB /255 ImageNet-norm (README)", recipe_A_imagenet),
        ("B: RGB raw 0-255 (config literal)", recipe_B_raw_rgb),
        ("C: BGR raw 0-255 (no color conv)", recipe_C_raw_bgr),
        ("D: RGB /255 no mean/std", recipe_D_scaled_rgb_no_meanstd),
    ]:
        blob, sh, sw = fn(img)
        dets = run(session, input_names, output_names, blob, sh, sw)
        summarize(name, dets)
        print()


if __name__ == "__main__":
    main()
