import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL_NAME = "tencent/Hunyuan-MT-7B"
DEVICE = "cuda" if torch.cuda.is_available() else "cpu"

tokenizer = AutoTokenizer.from_pretrained(
    MODEL_NAME,
    padding_side="left",
)

model = AutoModelForCausalLM.from_pretrained(
    MODEL_NAME,
    dtype=torch.float32 if DEVICE == "cpu" else torch.bfloat16,
)
model.eval()

try:
    if DEVICE == "cuda":
        model = torch.compile(model, mode="reduce-overhead")
except Exception:
    pass


def translate(
    source_lang: str,
    target_lang: str,
    texts: list[str],
) -> list[str]:
    messages_batch = [
        [
            {
                "role": "user",
                "content": (
                    f"Translate the following segment into {target_lang}, "
                    f"without additional explanation:\n\n{text}"
                ),
            }
        ]
        for text in texts
    ]

    batch_encoding = tokenizer.apply_chat_template(
        messages_batch,
        tokenize=True,
        add_generation_prompt=False,
        padding=True,
        return_tensors="pt",
    )
    input_ids = batch_encoding["input_ids"].to(model.device)
    attention_mask = batch_encoding["attention_mask"].to(model.device)

    input_len = input_ids.shape[1]

    with torch.inference_mode():
        output_ids = model.generate(
            input_ids,
            attention_mask=attention_mask,
            max_new_tokens=2048,
            do_sample=True,
            top_k=20,
            top_p=0.6,
            repetition_penalty=1.05,
            temperature=0.7,
            use_cache=True,
        )

    generated = output_ids[:, input_len:]

    translations = tokenizer.batch_decode(
        generated,
        skip_special_tokens=True,
    )

    return [t.strip() for t in translations]


if __name__ == "__main__":
    texts = [
        """Musealer Kabinettschrank, Nussbaum Massivholz, Nussbaum Maserholz,
        Nussbaum, Zwetschge, Ahorn auf Weichholz furniert.""",
        """Unterbau in Form eines Halbschrankes mit breit abgeschrägten,
        vorderen Ecken und geschnitzter Schlagleiste.""",
        """Art.Nr. G1419 Ölgemälde ‚The Education of the Virgin‘ nach Jules-Joseph Lefebvre, um 1900 in einer dekorativen Stuckrahmung

        Dieses Ölgemälde zeigt ein Figurenpaar, welches unter freiem Himmel an einer steinernen Brüstung auf einer Terrasse sitzen. Ein junges Mädchen kniet mit gesenktem Kopf und geschlossenen Augen vor einer älteren Dame. Sie trägt ein graues Gewand, sowie eine hellblaue Schleife in ihrem offenen gelockten Haar. Die Hände hält sie vor der Brust überkreuzt. Die Dame sitzt auf einer breiten, thronähnlichen Bank und ist bekleidet mit einem grün-braunen Gewand, das Haar ist bedeckt. Ihre rechte Hand hält sie erhoben, die linke verweist auf ein Schriftband auf ihrem Schoß. Beide werden von einem schmalen Heiligenschein bekrönt. Es handelt sich hierbei um eine Darstellung der Heiligen Anna, welche hier ihrer Tochter Maria das Lesen lehrt.

        Zustand: feine Craqueluren

        Jules-Joseph Lefebvre (1834-1912) war ein französischer Maler. Der Sohn eines Bäckers wurde in jungen Jahren nach Paris geschickt, um dort an der École nationale supérieure des beaux-arts de Paris zu studieren. Lefebvre stellte Werke im Pariser Salon aus, widmete sich dem Stil des Manierismus in Italien und kehrte nach wenigen Jahren nach Paris zurück. Dort erhielt er eine Lehrstelle an der Académie Julian. Zu seinen berühmtesten Schülern gehört u. a. der Symbolist Fernand Khnopff.""",
    ]

    translations = translate(
        source_lang="German",
        target_lang="English",
        texts=texts,
    )
    for i, (src, tgt) in enumerate(zip(texts, translations), 1):
        print(f"\n--- Item {i} ---")
        print(tgt.strip())
