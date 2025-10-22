from fastapi import FastAPI
from pydantic import BaseModel
from sentence_transformers import SentenceTransformer
import os

app = FastAPI()
device = os.getenv("DEVICE", "cpu")
model = SentenceTransformer("BAAI/bge-m3", device=device)  # force CPU for local dev

print(device)


class EmbedRequest(BaseModel):
    texts: list[str]


@app.post("/embed")
def embed(req: EmbedRequest):
    embeddings = model.encode(req.texts, normalize_embeddings=True, batch_size=4)
    return {"embeddings": embeddings.tolist()}
