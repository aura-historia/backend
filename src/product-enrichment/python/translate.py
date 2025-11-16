import os
import pathlib
from typing import List, Tuple

import ctranslate2
import torch
from cachetools import LRUCache
from transformers import AutoTokenizer

# ------------------------------------------------------------
# Configuration
# ------------------------------------------------------------
DEVICE = os.getenv(
    "HELSINKI_NLP_OPUS_MT_MODEL_DEVICE", "cuda" if torch.cuda.is_available() else "cpu"
)
DTYPE = "float16" if DEVICE == "cuda" else "float32"

CACHE_DIR = pathlib.Path("./models_cache")
CACHE_DIR.mkdir(exist_ok=True)


# Map (src, tgt) -> huggingface model name
MODEL_REGISTRY = {
    ("de", "en"): "Helsinki-NLP/opus-mt-de-en",
    ("de", "fr"): "Helsinki-NLP/opus-mt-de-fr",
    ("de", "es"): "Helsinki-NLP/opus-mt-de-es",
    ("en", "de"): "Helsinki-NLP/opus-mt-en-de",
    ("en", "fr"): "Helsinki-NLP/opus-mt-en-fr",
    ("en", "es"): "Helsinki-NLP/opus-mt-en-es",
    ("fr", "de"): "Helsinki-NLP/opus-mt-fr-de",
    ("fr", "en"): "Helsinki-NLP/opus-mt-fr-en",
    ("fr", "es"): "Helsinki-NLP/opus-mt-fr-es",
    ("es", "de"): "Helsinki-NLP/opus-mt-es-de",
    ("es", "en"): "Helsinki-NLP/opus-mt-es-en",
    ("es", "fr"): "Helsinki-NLP/opus-mt-es-fr",
}


# ------------------------------------------------------------
# LRU model cache (max 5 loaded models)
# ------------------------------------------------------------
_loaded_models: LRUCache[
    Tuple[str, str], Tuple[AutoTokenizer, ctranslate2.Translator]
] = LRUCache(maxsize=5)


def _load_model(src: str, tgt: str):
    key = (src, tgt)
    if key in _loaded_models:
        return _loaded_models[key]

    if key not in MODEL_REGISTRY:
        raise ValueError(f"No translation model registered for {src}->{tgt}")

    hf_model_name = MODEL_REGISTRY[key]

    tokenizer = AutoTokenizer.from_pretrained(hf_model_name)

    model_dir = CACHE_DIR / hf_model_name.replace("/", "_") / "ct2"

    # Ensure parent directory exists
    model_dir.parent.mkdir(parents=True, exist_ok=True)

    needs_conversion = not (model_dir / "model.bin").exists()
    lock_file = model_dir.parent / (model_dir.name + ".lock")

    if needs_conversion:
        if lock_file.exists():
            import time

            while lock_file.exists():
                time.sleep(1)
        else:
            with open(lock_file, "w"):
                pass
            try:
                print(f"[translate] Converting {hf_model_name} → CTranslate2...")
                import subprocess

                subprocess.run(
                    [
                        "ct2-transformers-converter",
                        "--model",
                        hf_model_name,
                        "--output_dir",
                        str(model_dir),
                        "--quantization",
                        "int8_float16" if DEVICE == "cuda" else "int8",
                        "--force",
                    ],
                    check=True,
                )
            finally:
                lock_file.unlink(missing_ok=True)

    ct2_model = ctranslate2.Translator(
        str(model_dir),
        device=DEVICE,
        compute_type=DTYPE,
    )

    _loaded_models[key] = (tokenizer, ct2_model)
    print(f"[translate] Loaded {hf_model_name} on {DEVICE} ({DTYPE})")

    return tokenizer, ct2_model


# ------------------------------------------------------------
# Public translate() function
# ------------------------------------------------------------
def translate(text: str, src: str, tgt: str) -> str:
    """Translate one text from src -> tgt."""
    tokenizer, ct2_model = _load_model(src, tgt)

    input_ids = tokenizer.encode(text, return_tensors=None)
    tokens = tokenizer.convert_ids_to_tokens(input_ids)
    result = ct2_model.translate_batch([tokens])[0]
    output_tokens = result.hypotheses[0]
    output_ids = tokenizer.convert_tokens_to_ids(output_tokens)
    return tokenizer.decode(output_ids, skip_special_tokens=True)


def translate_batch(texts: List[str], src: str, tgt: str) -> List[str]:
    tokenizer, ct2_model = _load_model(src, tgt)

    # Batch tokenization using HF tokenizer (Optimization 2)
    encoding = tokenizer(
        texts,
        return_tensors=None,
        padding=True,
        truncation=True,
    )
    batch_tokens = [
        tokenizer.convert_ids_to_tokens(ids) for ids in encoding["input_ids"]
    ]

    # Batch translation
    results = ct2_model.translate_batch(batch_tokens)

    # Decode each result
    translations = []
    for result in results:
        output_tokens = result.hypotheses[0]
        output_ids = tokenizer.convert_tokens_to_ids(output_tokens)
        decoded = tokenizer.decode(output_ids, skip_special_tokens=True)
        translations.append(decoded)

    return translations
