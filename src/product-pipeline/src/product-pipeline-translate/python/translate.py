from typing import List

from vllm import LLM, SamplingParams

llm = LLM(
    model="unsloth/Qwen3-14B-unsloth-bnb-4bit",
    dtype="bfloat16",
    max_model_len=4096,
    trust_remote_code=True,
    gpu_memory_utilization=0.95,
)

sampling_params = SamplingParams(
    temperature=0.1,
    top_p=1.0,
    max_tokens=1024,
    repetition_penalty=1.0,
)


def build_prompt(text: str, src_lang: str, tgt_lang: str) -> str:
    return f"""/no_think
        You are a professional translator specialized in antique and art-historical descriptions.
        Translate the following text faithfully and precisely.
        Do not summarize, omit, or add information.
        Preserve technical terminology.
        Do only answer with the translated text, nothing else.

        Source language: {src_lang}
        Target language: {tgt_lang}
        Text:\n{text}\n
        """


def translate(
    texts: List[str],
    source_language: str,
    target_language: str,
) -> List[str]:
    prompts = [build_prompt(text, source_language, target_language) for text in texts]

    outputs = llm.generate(
        prompts,
        sampling_params,
        extra_body={"top_k": 20, "chat_template_kwargs": {"enable_thinking": False}},
    )

    # vLLM guarantees order preservation
    translations = []
    for output in outputs:
        text = output.outputs[0].text.strip()
        translations.append(text)

    return translations


if __name__ == "__main__":
    german_texts = [
        "Barockes Hängeschränkchen aus massivem Eichenholz.",
        "Rechteckiger Korpus mit breitem einschübigem Sockelgeschoss.",
        "Gedrechselter Zugknopf an der Schubladenfront.",
    ]

    translated = translate(
        german_texts,
        source_language="German",
        target_language="English",
    )

    for src, tgt in zip(german_texts, translated):
        print(tgt)
        print("-" * 60)
