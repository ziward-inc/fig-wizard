#!/usr/bin/env python3
"""Render selected PDF pages to PNG images at a fixed DPI for the detection spike."""
import fitz
import os

OUT_DIR = "pages"
DPI = 200

# (pdf_name, page_index (0-based), output_tag)
TARGETS = [
    ("ppo", 4, "ppo_p05_algorithm"),
    ("attention", 2, "attention_p03_figure_arch"),
    ("attention", 3, "attention_p04_figure_attn"),
    ("attention", 5, "attention_p06_table"),
    ("attention", 7, "attention_p08_table"),
    ("resnet", 0, "resnet_p01_figure"),
    ("resnet", 3, "resnet_p04_table_figure"),
    ("resnet", 4, "resnet_p05_table_figure"),
    ("codex", 2, "codex_p03_code"),
    ("codex", 3, "codex_p04_code"),
    ("codex", 4, "codex_p05_code"),
    ("gpt3", 48, "gpt3_p49_quote_poem"),
    # Figure 1 of the CoT paper has genuine bordered/shaded callout boxes
    # ("Model Input" / "Model Output" panels) - this is the real quote/callout test.
    ("cot", 0, "cot_p01_callout"),
]

def main():
    os.makedirs(OUT_DIR, exist_ok=True)
    zoom = DPI / 72.0
    mat = fitz.Matrix(zoom, zoom)
    for pdf_name, page_idx, tag in TARGETS:
        doc = fitz.open(f"pdfs/{pdf_name}.pdf")
        page = doc[page_idx]
        pix = page.get_pixmap(matrix=mat)
        out_path = os.path.join(OUT_DIR, f"{tag}.png")
        pix.save(out_path)
        print(f"saved {out_path} ({pix.width}x{pix.height})")

if __name__ == "__main__":
    main()
