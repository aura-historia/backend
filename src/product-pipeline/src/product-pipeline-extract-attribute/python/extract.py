from typing import List

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL_NAME = "Qwen/Qwen3-8B"
DEVICE = "cuda" if torch.cuda.is_available() else "cpu"

tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)
model = AutoModelForCausalLM.from_pretrained(
    MODEL_NAME,
    device_map=DEVICE,
    dtype=torch.float16,
)

def extract(schema: str, texts: List[str], batch_size=8) -> List[str]:
    results: List[str] = []

    for i in range(0, len(texts), batch_size):
        batch = texts[i : i + batch_size]

        prompts = []
        for text in batch:
            messages = [
                {
                    "role": "system",
                    "content": (
                        "You are a structured information extraction system. "
                        "You extract product attributes from text. "
                        "You will be given a JSON-Schema to extract."
                        "You must respond with valid JSON only. "
                        "Do not include explanations or extra text."
                        "If values for any of the target schemas fields are missing, use null."
                    ),
                },
                {
                    "role": "user",
                    "content": f"""
                    Schema:
                    \"\"\"{schema}\"\"\"

                    Text:
                    \"\"\"{text}\"\"\"
                    """,
                },
            ]

            prompt = tokenizer.apply_chat_template(
                messages,
                tokenize=False,
                add_generation_prompt=True,
                enable_thinking=False,
            )
            prompts.append(prompt)

        inputs = tokenizer(
            prompts,
            return_tensors="pt",
            padding=True,
            truncation=True,
        ).to(model.device)

        with torch.inference_mode():
            outputs = model.generate(
                **inputs,
                max_new_tokens=100,
                do_sample=False,
            )

        input_length = inputs["input_ids"].shape[1]

        decoded = tokenizer.batch_decode(
            outputs[:, input_length:],
            skip_special_tokens=True,
        )

        results.extend(decoded)

    return results
