import os

import numpy as np
import torch
from sentence_transformers import SentenceTransformer

DEVICE = os.getenv(
    "BAAI_BGE_M3_MODEL_DEVICE", "cuda" if torch.cuda.is_available() else "cpu"
)
model = SentenceTransformer("BAAI/bge-m3", device=DEVICE)
print(f"[embed] Loaded BAAI/bge-m3 on {DEVICE}")


def embed(texts):
    embeddings = model.encode(texts, normalize_embeddings=True, batch_size=64)
    return embeddings.astype(np.float32).tolist()
