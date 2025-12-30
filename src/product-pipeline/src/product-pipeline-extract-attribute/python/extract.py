import torch
from transformers import AutoModelForCausalLM, AutoTokenizer, BitsAndBytesConfig

MODEL_NAME = "Qwen/Qwen3-14B"
DEVICE = "cuda" if torch.cuda.is_available() else "cpu"

tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)
quant_config = BitsAndBytesConfig(
    load_in_8bit=True,
    llm_int8_threshold=6.0,
    llm_int8_has_fp16_weight=True,
)
model = AutoModelForCausalLM.from_pretrained(
    MODEL_NAME,
    device_map=DEVICE,
    dtype=torch.float16,
    quantization_config=quant_config,
)


def process_products(descriptions, batch_size=4):
    results = []

    for i in range(0, len(descriptions), batch_size):
        batch = descriptions[i : i + batch_size]

        prompts = []
        for desc in batch:
            messages = [
                {
                    "role": "user",
                    "content": f"""
Extract the following attributes from the product description:
- Brand
- Material
- Color
- Size

Product Description:
\"\"\"{desc}\"\"\"

Return strict JSON only.
""",
                }
            ]

            prompt = tokenizer.apply_chat_template(
                messages,
                tokenize=False,
                add_generation_prompt=True,
                enable_thinking=False,
                Temperature=0.7,
                TopP=0.8,
                TopK=20,
                MinP=0,
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
                max_new_tokens=150,
                do_sample=False,
            )

        input_length = inputs["input_ids"].shape[1]

        decoded = tokenizer.batch_decode(
            outputs[:, input_length:],
            skip_special_tokens=True,
        )

        results.extend(decoded)

    return results


# === 3. Example usage ===
if __name__ == "__main__":
    sample_descriptions = [
        "Nike Air Max 270, black color, size 10, made of synthetic leather.",
    ]

    responses = process_products(sample_descriptions, batch_size=2)
    for r in responses:
        print(r)
