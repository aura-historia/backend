from typing import List, Tuple

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL_NAME = (
    "unsloth/Qwen3-8B-unsloth-bnb-4bit"
    if torch.cuda.is_available()
    else "Qwen/Qwen3-1.7B"
)
DEVICE = "cuda" if torch.cuda.is_available() else "cpu"

tokenizer = AutoTokenizer.from_pretrained(MODEL_NAME)
model = AutoModelForCausalLM.from_pretrained(
    MODEL_NAME,
    device_map=DEVICE,
    dtype=torch.float32 if DEVICE == "cpu" else torch.float16,
    tie_word_embeddings=False,
)


def classify_category(
    batch: List[Tuple[str, List[str]]],
    batch_size: int = 64,
) -> List[str]:
    """
    Args:
        batch: List of tuples:
            [
                (product_title, [candidate1, candidate2, ...]),
                ...
            ]
        batch_size: inference batch size

    Returns:
        List[str]: predicted category_id for each product
    """

    results: List[str] = []

    for i in range(0, len(batch), batch_size):
        mini_batch = batch[i : i + batch_size]
        prompts = []

        for product_title, candidates in mini_batch:
            # Format candidate list into clean bullet list
            formatted_candidates = "\n".join(f"- {c}" for c in candidates)

            messages = [
                {
                    "role": "system",
                    "content": (
                        "You are a tie-breaker for antiques-product category-classification. "
                        "You will be given a product-title of an antique and multiple options for the category. "
                        "You MUST choose the best fitting category for the product. "
                        "Respond ONLY with the value of the matching kebab-case category."
                    ),
                },
                {
                    "role": "user",
                    "content": (
                        f"Product Title:\n"
                        f'"""{product_title}"""\n\n'
                        f"Candidate Categories:\n"
                        f"{formatted_candidates}"
                    ),
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
                max_new_tokens=32,
                do_sample=False,
            )

        input_length = inputs["input_ids"].shape[1]

        decoded = tokenizer.batch_decode(
            outputs[:, input_length:],
            skip_special_tokens=True,
        )

        # Strip whitespace and remove surrounding quotes only
        results.extend([d.strip().strip('"') for d in decoded])

    return results


if __name__ == "__main__":
    batch = [
        (
            """Musealer Kabinettschrank 1742, süddeutsch  Art.Nr. 6948""",
            ["musical-instruments", "furniture", "decorative-objects"],
        ),
    ]

    results = classify_category(batch)

    for i, (src, res) in enumerate(zip(batch, results), 1):
        print(f"\n--- Item {i} ---")
        print(res)
