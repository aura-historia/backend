from typing import List

from vllm import LLM, SamplingParams

llm = LLM(
    model="unsloth/Qwen3-14B-unsloth-bnb-4bit",
    dtype="bfloat16",
    max_model_len=1024,
    trust_remote_code=True,
    gpu_memory_utilization=0.95,
)

sampling_params = SamplingParams(
    temperature=0.1,
    top_p=1.0,
    max_tokens=512,
    repetition_penalty=1.0,
)


def build_prompt(schema: str, text: str) -> str:
    return f"""/no_think
        You are a structured information extraction system for the antiques and art domain.
        You extract product attributes from text.
        You will be given a JSON-Schema to extract.
        You must respond with valid JSON only.
        Do not include explanations or extra text.
        Only answer with the extracted JSON.
        If values for any of the target-schema fields are missing, use null.

        Schema:\n{schema}\n

        Text:\n{text}\n
        """


def extract(schema: str, texts: List[str]) -> List[str]:
    prompts = [build_prompt(schema, text) for text in texts]

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
