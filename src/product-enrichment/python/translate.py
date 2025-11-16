import os
import pathlib
from typing import List

import ctranslate2
import torch
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
# Model cache
# We keep loaded tokenizer + ct2 model in memory
# ------------------------------------------------------------
_loaded_models = {}


def _load_model(src: str, tgt: str):
    key = (src, tgt)
    if key in _loaded_models:
        return _loaded_models[key]

    if key not in MODEL_REGISTRY:
        raise ValueError(f"No translation model registered for {src}->{tgt}")

    hf_model_name = MODEL_REGISTRY[key]
    tokenizer = AutoTokenizer.from_pretrained(hf_model_name)
    model_dir = CACHE_DIR / hf_model_name.replace("/", "_") / "ct2"
    needs_conversion = not (model_dir / "model.bin").exists()

    if needs_conversion:
        print(f"[translate] Converting {hf_model_name} → CTranslate2...")

        # IMPORTANT: ensure parent exists, but don't pre-create the ct2 directory
        model_dir.parent.mkdir(parents=True, exist_ok=True)

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
                "--force",  # allow existing directory
            ],
            check=True,
        )

    # Load CT2 model
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
    """Translate a batch of texts from src -> tgt."""
    tokenizer, ct2_model = _load_model(src, tgt)

    batch_tokens = []
    for text in texts:
        input_ids = tokenizer.encode(text, return_tensors=None)
        tokens = tokenizer.convert_ids_to_tokens(input_ids)
        batch_tokens.append(tokens)

    results = ct2_model.translate_batch(batch_tokens)

    translations = []
    for result in results:
        output_tokens = result.hypotheses[0]
        output_ids = tokenizer.convert_tokens_to_ids(output_tokens)
        decoded = tokenizer.decode(output_ids, skip_special_tokens=True)
        translations.append(decoded)

    return translations


texts = ["Hallo Welt", "Dies ist ein Test.", "Wie geht es dir?"]

translations = translate_batch(texts, src="de", tgt="en")
for t in translations:
    print(t)
