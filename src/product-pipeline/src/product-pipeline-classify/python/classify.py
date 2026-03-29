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


def classify(
    batch: List[Tuple[str, List[str], List[str]]],
    batch_size: int = 64,
) -> List[Tuple[str, str]]:
    """
    Args:
        batch: List of tuples:
            [
                (product_title, [candidate_category1, ...], [candidate_period1, ...]),
                ...
            ]
        batch_size: inference batch size

    Returns:
        List[Tuple[str, str]]: predicted (category_id, period_id) for each product
    """

    results: List[Tuple[str, str]] = []

    for i in range(0, len(batch), batch_size):
        mini_batch = batch[i : i + batch_size]
        prompts = []

        for product_title, candidate_categories, candidate_periods in mini_batch:
            formatted_categories = "\n".join(f"- {c}" for c in candidate_categories)
            formatted_periods = "\n".join(f"- {p}" for p in candidate_periods)

            messages = [
                {
                    "role": "system",
                    "content": (
                        "You are a classifier for antiques products. "
                        "You will be given a product-title of an antique, "
                        "multiple options for the category, and multiple options for the period or style. "
                        "You MUST choose the best fitting category AND the best fitting period for the product. "
                        "Respond ONLY with two lines:\n"
                        "category: <chosen kebab-case category>\n"
                        "period: <chosen kebab-case period>"
                    ),
                },
                {
                    "role": "user",
                    "content": (
                        f"Product Title:\n"
                        f'"""{product_title}"""\n\n'
                        f"Candidate Categories:\n"
                        f"{formatted_categories}\n\n"
                        f"Candidate Periods:\n"
                        f"{formatted_periods}"
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
                max_new_tokens=64,
                do_sample=False,
            )

        input_length = inputs["input_ids"].shape[1]

        decoded = tokenizer.batch_decode(
            outputs[:, input_length:],
            skip_special_tokens=True,
        )

        for d in decoded:
            category = ""
            period = ""
            for line in d.strip().splitlines():
                line = line.strip()
                if line.lower().startswith("category:"):
                    category = line.split(":", 1)[1].strip().strip('"')
                elif line.lower().startswith("period:"):
                    period = line.split(":", 1)[1].strip().strip('"')
            results.append((category, period))

    return results


if __name__ == "__main__":
    batch = [
        (
            """Musealer Kabinettschrank 1742, süddeutsch  Art.Nr. 6948""",
            ["musical-instruments", "furniture", "decorative-objects"],
            ["renaissance", "baroque", "rococo"],
        ),
    ]

    results = classify(batch)

    for i, (src, res) in enumerate(zip(batch, results), 1):
        print(f"\n--- Item {i} ---")
        print(f"category: {res[0]}")
        print(f"period: {res[1]}")
