import numpy as np
from sentence_transformers import SentenceTransformer
import os

device = os.getenv("BAAI_BGE_M3_MODEL_DEVICE", "cpu")
model = SentenceTransformer("BAAI/bge-m3", device=device)
print(f"Using device '{device}'.")


def embed(texts):
    embeddings = model.encode(texts, normalize_embeddings=True, batch_size=64)
    return embeddings.astype(np.float32).tolist()
