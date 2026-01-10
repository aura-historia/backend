from typing import List

import torch
from transformers import AutoModelForCausalLM, AutoTokenizer

MODEL_NAME = "unsloth/Qwen3-14B-unsloth-bnb-4bit"
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
                        "If values for any of the target schemas fields are missing, use null. "
                        "If your confidence for values of certain fields is below 80%, use null."
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
                max_new_tokens=256,
                do_sample=False,
            )

        input_length = inputs["input_ids"].shape[1]

        decoded = tokenizer.batch_decode(
            outputs[:, input_length:],
            skip_special_tokens=True,
        )

        results.extend(decoded)

    return results


if __name__ == "__main__":
    schema = """
    {
        "originYearMin": int | null (Lower end of the year-range, the antique is from),
        "originYearMax": int | null (Higher end of the year-range, the antique is from),
        "originYear": int | null (Exact year the antique is from),
        "authenticity": enum-string | null (The authenticity of the antique. Either of: ORIGINAL, LATER_COPY (antique copy), REPRODUCTION (modern copy), QUESTIONABLE, UNKNOWN),
        "condition": enum-string | null (The condition of the antique. Either of: EXCELLENT, GREAT, GOOD, FAIR, POOR, UNKNOWN),
        "provenance": enum-string | null (The documentation (trail) of the antique. Either of: COMPLETE, PARTIAL, CLAIMED (assumed, but no proof), NONE, UNKNOWN),
        "restoration": enum-string | null (Restoration done to the antique. Either of: MAJOR, MINOR, NONE, UNKNOWN)
    }
    """

    texts = [
        """Musealer Kabinettschrank 1742, süddeutsch  Art.Nr. 6948
        Musealer Kabinettschrank, Nussbaum Massivholz, Nussbaum Maserholz, Nussbaum, Zwetschge, Ahorn auf Weichholz furniert. Unterbau in Form eines Halbschrankes mit breit abgeschrägten, vorderen Ecken. In der Mitte mit vorgesetzter, breiter, geschnitzter Schlagleiste, die in der Mitte mit einem Puttenkopf verziert ist, flankiert von Akanthusblättern.
        Vorgesetzter Sockel auf gedrückten Kugelfüßen, in der Mitte verkröpft. Der Unterbau schließt nach oben hin mit zwei Schubfächern ab, die Front ist in eine breite Kehle eingelassen, die am oberen und unteren Ansatz von Profilen begrenzt wird. Der zurückgesetzte Aufsatz hat seitlich vorgesetzte gewundene Vollsäulen und ist in drei Felder unterteilt. Die Pilaster übernehmen die Form der Schlagleiste des Unterbaus. Sehr aufwendiger Schnitzdekor, der sich beispielsweise in den korinthischen Kapitellen der Säulen zeigt.
        Das aufwendige Gesims ist in der Mitte sowie an den vorderen Ecken verkröpft. Am unteren Ansatz mit einem mehrfach abgetreppten Profil dekoriert. Der Gesimskranz ist außergewöhnlich, breit ausgestellt und mit einer breiten Kehlung, stehend furniert und mit einem stark profilierten, oberen Abschluss.
        Bei geöffneten Türen des Aufbaus kann die mittige Blende entriegelt und nach rechts verschoben werden, so dass vier Geheimfächer zugängig werden (ursprünglich 5 Schübe). Die Schubladen, die Seitenteile sind dekoriert mit Feldern aus Wurzelmaserfurnier, umrahmt von Bändern in Zwetschgenholz, flankiert von Ahornadern. Den äußeren Rahmen bilden schräg angelegte Bänder aus Nussbaum, teils fischgradförmig angelegt.
        Originale getriebene Messingbeschläge, originale Schlösser in Eisen. Die Innenflächen teils mit einer Tapete aus dem 19. Jahrhundert ausgekleidet. Die Türen sind im Innenbereich farblich gefasst, mit einem gekämmten Dekor. Die Beschläge, die Schlösser sind mit handgefeilten Schrauben sowie geschmiedeten Nägeln fixiert.

        Restaurierung
        Sehr guter Erhaltungszustand, minimale, altersbedingte Gebrauchsspuren, kleinere Kratzer, Druckstellen, die gereinigt und überpoliert wurden. Füße im 19. Jahrhundert ergänzt. Minimale Spuren alten Holzwurmbefalls, primär am linken Sockel, die kaum auffallen.
        Süddeutsch, 1. Hälfte 18. Jahrhundert, Ergänzungen, Füße, Tapete aus dem 19. Jahrhundert.
        Literaturvergleich: Barockmöbel Uwe Dobler
        Höhe: 201, Höhe Unterbau: 101 cm
        Breite: 172 cm
        Tiefe: 75 cm
        """,
    ]

    results = extract(schema, texts)

    for i, (src, res) in enumerate(zip(texts, results), 1):
        print(f"\n--- Item {i} ---")
        print(res)
