# Install dependencies:
# pip install numpy opencv-python onnxruntime

import numpy as np
import cv2
import onnxruntime as ort
from pathlib import Path

def preprocess_image_doclayout(image, target_input_size=(800, 800)):
    """
    Preprocessing for DocLayoutV3 with 800x800 input.
    Returns CHW tensor (no batch dim) + scale factors.
    """
    orig_h, orig_w = image.shape[:2]
    target_h, target_w = target_input_size
    scale_h = target_h / orig_h
    scale_w = target_w / orig_w

    resized = cv2.resize(image, (target_w, target_h), interpolation=cv2.INTER_LINEAR)
    rgb = cv2.cvtColor(resized, cv2.COLOR_BGR2RGB)
    blob = rgb.astype(np.float32) / 255.0

    mean = np.array([0.485, 0.456, 0.406], dtype=np.float32)
    std  = np.array([0.229, 0.224, 0.225], dtype=np.float32)
    blob = (blob - mean) / std

    # CHW — no batch dim yet; caller stacks the batch
    blob = blob.transpose(2, 0, 1)
    return blob, scale_h, scale_w


def preprocess_batch(image_paths, target_input_size=(800, 800)):
    """
    Load and preprocess a list of image paths.
    Returns:
        input_blob  : (N, 3, H, W)  float32
        shape_list  : (N, 2)        float32  [[H, W], ...]
        scale_list  : (N, 2)        float32  [[scale_h, scale_w], ...]
        images      : list of original BGR images (for debug / visualisation)
    """
    blobs, shapes, scales, images = [], [], [], []

    for path in image_paths:
        img = cv2.imread(str(path))
        if img is None:
            raise FileNotFoundError(f"Could not read image: {path}")

        blob, scale_h, scale_w = preprocess_image_doclayout(img, target_input_size)
        blobs.append(blob)
        shapes.append(target_input_size)          # (H, W)
        scales.append((scale_h, scale_w))
        images.append(img)

    input_blob = np.stack(blobs, axis=0).astype(np.float32)        # (N, 3, H, W)
    shape_arr  = np.array(shapes, dtype=np.float32)                 # (N, 2)
    scale_arr  = np.array(scales, dtype=np.float32)                 # (N, 2)

    return input_blob, shape_arr, scale_arr, images


def run_doclayout_onnx_batch(image_paths, model_path, conf_thresh=0.5):
    """
    Run DocLayoutV3 on a batch of images.

    The model's three inputs are:
        input_names[0] : image shape   – expected shape (N, 2)
        input_names[1] : image tensor  – expected shape (N, 3, H, W)
        input_names[2] : scale factors – expected shape (N, 2)

    Output shape: (N * max_dets, 7)
    Values: [image_index, label_index, score, xmin, ymin, xmax, ymax]
    (Some ONNX exports omit image_index — see note in post-processing.)
    """
    model = ort.InferenceSession(model_path)
    input_names  = [i.name for i in model.get_inputs()]
    output_names = [o.name for o in model.get_outputs()]

    input_blob, shape_arr, scale_arr, images = preprocess_batch(image_paths)
    n = len(image_paths)

    input_feed = {
        "im_shape": shape_arr,  # (N, 2)
        "image": input_blob,  # (N, 3, 800, 800)
        "scale_factor": scale_arr,  # (N, 2)
    }

    raw_output = model.run(output_names, input_feed)[0]  # (N*dets, 7) or (N*dets, 6)

    return postprocess_batch(raw_output, n, conf_thresh)


def postprocess_batch(raw_output, n_images, conf_thresh=0.5):
    """
    Split flat detection output back into per-image results.

    PP-DocLayout ONNX output columns:
        [img_idx, label, score, x0, y0, x1, y1, read_order]   (8 cols)
      or
        [label, score, x0, y0, x1, y1, read_order]             (7 cols — single-image compat)

    We handle both layouts automatically.
    """
    n_cols = raw_output.shape[1]

    if n_cols == 8:
        # Batched export: first column is the image index
        img_idx_col = raw_output[:, 0].astype(int)
        detections  = raw_output[:, 1:]   # drop img_idx → 7 cols
    else:
        # Single-image export used for a batch: distribute evenly
        dets_per_image = len(raw_output) // n_images
        img_idx_col = np.repeat(np.arange(n_images), dets_per_image)
        detections  = raw_output

    results = []
    for i in range(n_images):
        mask  = img_idx_col == i
        boxes = detections[mask]
        boxes = boxes[boxes[:, 1] > conf_thresh]          # confidence filter
        boxes = boxes[np.argsort(boxes[:, 6])]            # sort by read_order
        results.append(boxes)

    return results


def print_doclayout_res(boxes, image_label=""):
    header = f"--- {image_label} ---" if image_label else "--- Results ---"
    print(header)
    print("cls_id\tscore\txmin\tymin\txmax\tymax\tread_order")
    for box in boxes:
        print(
            f"{box[0]:.0f}\t\t{box[1]:.3f}\t"
            f"{box[2]:.2f}\t{box[3]:.2f}\t"
            f"{box[4]:.2f}\t{box[5]:.2f}\t{box[6]:.0f}"
        )


if __name__ == '__main__':
    MODEL_PATH = "your/path/to/PP-DocLayoutV3.onnx"

    image_paths = [
        "your_test_image_1.png",
        "your_test_image_2.png",
        "your_test_image_3.png",
        "your_test_image_4.png",
    ]

    results = run_doclayout_onnx_batch(image_paths, MODEL_PATH, conf_thresh=0.5)

    for path, boxes in zip(image_paths, results):
        print_doclayout_res(boxes, image_label=Path(path).name)
        print()
