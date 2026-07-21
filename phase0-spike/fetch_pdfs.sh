#!/bin/bash
# Downloads the arXiv papers used as Phase-0 detection test cases.
# Re-run this if pdfs/ is missing or you want to regenerate from scratch.
set -euo pipefail
mkdir -p pdfs
cd pdfs

# PPO paper - has a classic "Algorithm 1" pseudocode box (page index 4)
curl -sL -o ppo.pdf "https://arxiv.org/pdf/1707.06347"

# Attention Is All You Need - two-column, figures/tables/formulas
curl -sL -o attention.pdf "https://arxiv.org/pdf/1706.03762"

# Deep Residual Learning (ResNet) - two-column, figures (line charts) + tables
curl -sL -o resnet.pdf "https://arxiv.org/pdf/1512.03385"

# Adam optimizer paper - formulas, algorithm box (not used in final test set but handy)
curl -sL -o adam.pdf "https://arxiv.org/pdf/1412.6980"

# Codex (Evaluating LLMs Trained on Code) - real monospace Python code-listing figures
curl -sL -o codex.pdf "https://arxiv.org/pdf/2107.03374"

# GPT-3 paper - used to probe for "quote"-like content (inconclusive test case, see report)
curl -sL -o gpt3.pdf "https://arxiv.org/pdf/2005.14165"

# Chain-of-Thought Prompting - Figure 1 has genuine bordered/shaded callout boxes
# ("Model Input" / "Model Output" panels) - our best real quote/callout test case
curl -sL -o cot.pdf "https://arxiv.org/pdf/2201.11903"

echo "Done. PDFs saved to pdfs/"
