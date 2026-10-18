//! Explicit item statements, never translations or substring guesses at runtime.
use localization::Language;

pub(super) struct LanguagePhrases {
    pub language: Language,
    /// Whole statements, one per line.
    pub prohibited: &'static str,
    pub collection_only: &'static str,
    pub worldwide: &'static str,
    pub on_request: &'static str,
    pub calculated: &'static str,
    /// Prefix/suffix include their own whitespace. Every complete expression is anchored.
    pub dispatch_prefix: &'static str,
    pub range_separator: &'static str,
    pub dispatch_suffix: &'static str,
    /// Word prefixes are used only to abstain on unrecognized delivery clauses.
    pub delivery_words: &'static [&'static str],
    /// For scripts without word separators; these markers can only veto an assertion.
    pub delivery_markers: &'static [&'static str],
    pub conditions: &'static [&'static str],
    pub condition_markers: &'static [&'static str],
}

pub(super) const LANGUAGES: &[LanguagePhrases] = &[
    LanguagePhrases {
        language: Language::En,
        prohibited: "shipping is not available for this item\n\
            no shipping for this item\n\
            this item cannot be shipped\n\
            this item is collection only\n\
            this item is pickup only",
        collection_only: "collection only\n\
            pickup only\n\
            pick-up only",
        worldwide: "shipping for this item is available worldwide\n\
            this item can be shipped worldwide",
        on_request: "shipping costs for this item worldwide on request\n\
            worldwide shipping costs for this item are available on request",
        calculated: "shipping costs for this item worldwide are calculated at checkout\n\
            worldwide shipping costs for this item are calculated at checkout",
        dispatch_prefix: r"(?:dispatch(?: for this item)? in|this item will be dispatched within)\s+",
        range_separator: "-|to",
        dispatch_suffix: r"\s+(?:business|working) days?",
        delivery_words: &[
            "ship", "deliver", "dispatch", "pickup", "postage", "freight",
        ],
        delivery_markers: &["collection only", "pick up", "pick-up"],
        conditions: &["if", "unless", "except", "however", "but", "subject to"],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::De,
        prohibited: "kein versand für diesen artikel\n\
            dieser artikel kann nicht versendet werden\n\
            dieser artikel ist nur zur selbstabholung verfügbar\n\
            dieser artikel ist nur zur abholung verfügbar",
        collection_only: "nur selbstabholung\n\
            nur abholung",
        worldwide: "weltweiter versand für diesen artikel möglich\n\
            dieser artikel kann weltweit versendet werden",
        on_request: "weltweite versandkosten für diesen artikel auf anfrage\n\
            weltweite versandkosten für diesen artikel erhalten sie auf anfrage",
        calculated: "weltweite versandkosten für diesen artikel werden an der kasse berechnet\n\
            weltweite versandkosten für diesen artikel werden beim checkout berechnet",
        dispatch_prefix: r"(?:versandfertig in|dieser artikel ist versandfertig in)\s+",
        range_separator: "-|bis",
        dispatch_suffix: r"\s+werktagen",
        delivery_words: &["versand", "versend", "liefer", "abhol", "werktag"],
        delivery_markers: &[],
        conditions: &[
            "wenn",
            "falls",
            "außer",
            "ausser",
            "jedoch",
            "aber",
            "sofern",
            "es sei denn",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Fr,
        prohibited: "aucune expédition pour cet article\n\
            cet article ne peut pas être expédié\n\
            cet article est disponible uniquement en retrait sur place\n\
            cet article est à retirer sur place uniquement",
        collection_only: "retrait sur place uniquement\n\
            retrait uniquement\n\
            remise en main propre uniquement",
        worldwide: "expédition dans le monde entier disponible pour cet article\n\
            cet article peut être expédié dans le monde entier",
        on_request: "frais d'expédition dans le monde entier pour cet article sur demande\n\
            les frais d'expédition dans le monde entier pour cet article sont disponibles sur demande",
        calculated: "les frais d'expédition dans le monde entier pour cet article sont calculés lors du paiement\n\
            frais d'expédition dans le monde entier pour cet article calculés au paiement",
        dispatch_prefix: r"(?:expédition(?: de cet article)? sous|cet article sera expédié sous)\s+",
        range_separator: "-|à|a",
        dispatch_suffix: r"\s+(?:jours? ouvrés?|jours? ouvrables?)",
        delivery_words: &["expédi", "expedi", "expédit", "livr", "retrait", "envoi"],
        delivery_markers: &["remise en main propre"],
        conditions: &[
            "si",
            "sauf",
            "excepté",
            "exceptée",
            "à condition",
            "sous réserve",
            "mais",
            "toutefois",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Es,
        prohibited: "no se realizan envíos de este artículo\n\
            este artículo no se puede enviar\n\
            este artículo solo está disponible para recogida en persona\n\
            este artículo es solo para recogida",
        collection_only: "solo recogida\n\
            solo recogida en persona\n\
            únicamente recogida en persona",
        worldwide: "este artículo se puede enviar a todo el mundo\n\
            envío a todo el mundo disponible para este artículo",
        on_request: "gastos de envío a todo el mundo para este artículo bajo consulta\n\
            gastos de envío a todo el mundo para este artículo a consultar",
        calculated: "los gastos de envío a todo el mundo para este artículo se calculan al finalizar la compra\n\
            gastos de envío a todo el mundo para este artículo calculados al pagar",
        dispatch_prefix: r"(?:envío(?: de este artículo)? en|este artículo se enviará en)\s+",
        range_separator: "-|a",
        dispatch_suffix: r"\s+(?:días (?:hábiles|laborables)|día (?:hábil|laborable))",
        delivery_words: &["enví", "envi", "entreg", "recog", "transporte"],
        delivery_markers: &[],
        conditions: &[
            "si",
            "salvo",
            "excepto",
            "exceptuando",
            "a menos que",
            "pero",
            "sin embargo",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::It,
        prohibited: "nessuna spedizione per questo articolo\n\
            questo articolo non può essere spedito\n\
            questo articolo è disponibile solo per il ritiro a mano\n\
            questo articolo è disponibile solo per il ritiro in sede",
        collection_only: "solo ritiro a mano\n\
            solo ritiro in sede\n\
            solo ritiro",
        worldwide: "questo articolo può essere spedito in tutto il mondo\n\
            spedizione in tutto il mondo disponibile per questo articolo",
        on_request: "spese di spedizione in tutto il mondo per questo articolo su richiesta\n\
            costi di spedizione in tutto il mondo per questo articolo su richiesta",
        calculated: "le spese di spedizione in tutto il mondo per questo articolo vengono calcolate al pagamento\n\
            spese di spedizione in tutto il mondo per questo articolo calcolate al checkout",
        dispatch_prefix: r"(?:spedizione(?: di questo articolo)? entro|questo articolo sarà spedito entro)\s+",
        range_separator: "-|a",
        dispatch_suffix: r"\s+(?:giorni lavorativi|giorno lavorativo)",
        delivery_words: &["sped", "consegn", "ritiro", "trasporto"],
        delivery_markers: &[],
        conditions: &[
            "se",
            "salvo",
            "eccetto",
            "tranne",
            "a condizione",
            "a meno che",
            "ma",
            "tuttavia",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Zh,
        prohibited: "本商品不支持配送\n\
            此商品不提供配送\n\
            本商品仅支持自提\n\
            此商品仅限自提\n\
            本商品不提供寄送\n\
            此商品不提供寄送\n\
            本商品僅供自取\n\
            此商品僅限自取",
        collection_only: "仅限自提\n\
            仅支持自提\n\
            僅限自取\n\
            僅供自取",
        worldwide: "本商品支持全球配送\n\
            此商品可配送至全球\n\
            本商品提供全球寄送\n\
            此商品可寄送至全球",
        on_request: "本商品的全球配送运费需询价\n\
            此商品的全球配送运费请咨询\n\
            本商品的全球寄送運費需詢價\n\
            此商品的全球寄送運費請洽詢",
        calculated: "本商品的全球配送运费在结账时计算\n\
            此商品的全球配送运费于结账时计算\n\
            本商品的全球寄送運費於結帳時計算\n\
            此商品的全球寄送運費在結帳時計算",
        dispatch_prefix: r"(?:本商品将在|此商品将在|本商品將於|此商品將於|预计|預計)\s*",
        range_separator: "-|至|到",
        dispatch_suffix: r"\s*(?:个?工作日内发货|個?工作日內發貨|個?工作天內出貨|个?工作日内发出|個?工作日內寄出)",
        delivery_words: &[],
        delivery_markers: &[
            "配送",
            "发货",
            "發貨",
            "出货",
            "出貨",
            "发出",
            "發出",
            "寄送",
            "运费",
            "運費",
            "自提",
            "自取",
            "送达",
            "送達",
            "到货",
            "到貨",
            "工作日",
            "工作天",
        ],
        conditions: &[],
        condition_markers: &[
            "如果",
            "若",
            "除非",
            "但是",
            "但需",
            "但須",
            "例外",
            "视情况",
            "視情況",
        ],
    },
    LanguagePhrases {
        language: Language::Pt,
        prohibited: "não fazemos envios deste artigo\n\
            este artigo não pode ser enviado\n\
            este produto não pode ser enviado\n\
            este artigo está disponível apenas para recolha em mãos\n\
            este produto está disponível apenas para retirada no local",
        collection_only: "apenas recolha em mãos\n\
            apenas recolha no local\n\
            somente retirada no local\n\
            apenas retirada",
        worldwide: "este artigo pode ser enviado para todo o mundo\n\
            este produto pode ser enviado para todo o mundo",
        on_request: "custos de envio para todo o mundo deste artigo sob consulta\n\
            frete para todo o mundo deste produto sob consulta",
        calculated: "os custos de envio para todo o mundo deste artigo são calculados no checkout\n\
            o frete para todo o mundo deste produto é calculado ao finalizar a compra",
        dispatch_prefix: r"(?:expedição(?: deste artigo| deste produto)? em|este artigo será expedido em|este produto será enviado em)\s+",
        range_separator: "-|a",
        dispatch_suffix: r"\s+(?:dias úteis|dia útil)",
        delivery_words: &[
            "envi",
            "expedi",
            "entreg",
            "recolh",
            "retirad",
            "frete",
            "transporte",
        ],
        delivery_markers: &[],
        conditions: &[
            "se",
            "salvo",
            "exceto",
            "excepto",
            "a menos que",
            "mas",
            "porém",
            "sob condição",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Pl,
        prohibited: "wysyłka tego przedmiotu nie jest dostępna\n\
            tego przedmiotu nie można wysłać\n\
            ten przedmiot jest dostępny wyłącznie z odbiorem osobistym\n\
            ten przedmiot jest dostępny tylko z odbiorem osobistym",
        collection_only: "tylko odbiór osobisty\n\
            wyłącznie odbiór osobisty",
        worldwide: "ten przedmiot można wysłać na cały świat\n\
            wysyłka tego przedmiotu jest dostępna na cały świat",
        on_request: "koszty wysyłki tego przedmiotu na cały świat na zapytanie\n\
            koszt wysyłki tego przedmiotu na cały świat na zapytanie",
        calculated: "koszty wysyłki tego przedmiotu na cały świat są obliczane przy kasie\n\
            koszt wysyłki tego przedmiotu na cały świat jest obliczany przy kasie",
        dispatch_prefix: r"(?:wysyłka(?: tego przedmiotu)? w ciągu|ten przedmiot zostanie wysłany w ciągu)\s+",
        range_separator: "-|do",
        dispatch_suffix: r"\s+(?:dni roboczych|dnia roboczego)",
        delivery_words: &[
            "wysył", "wysła", "dostaw", "odbiór", "odbiorem", "przesył", "roboczy",
        ],
        delivery_markers: &[],
        conditions: &[
            "jeśli",
            "jeżeli",
            "chyba że",
            "z wyjątkiem",
            "pod warunkiem",
            "ale",
            "jednak",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Tr,
        prohibited: "bu ürün için kargo hizmeti verilmez\n\
            bu ürün için gönderim mevcut değil\n\
            bu ürün sadece elden teslim edilebilir\n\
            bu ürün sadece mağazadan teslim alınabilir",
        collection_only: "sadece elden teslim\n\
            sadece mağazadan teslim",
        worldwide: "bu ürün için dünya genelinde gönderim mevcuttur\n\
            bu ürün için dünya çapında gönderim mevcuttur",
        on_request: "bu ürün için dünya genelinde kargo ücreti talep üzerine bildirilir\n\
            bu ürün için dünya çapında kargo ücreti talep üzerine bildirilir",
        calculated: "bu ürün için dünya genelinde kargo ücreti kasada belirlenir\n\
            bu ürün için dünya çapında kargo ücreti kasada belirlenir",
        dispatch_prefix: r"(?:bu ürün\s+)?",
        range_separator: "-|ila|ile",
        dispatch_suffix: r"\s+iş günü içinde kargoya verilir",
        delivery_words: &["kargo", "gönder", "teslim", "nakliye"],
        delivery_markers: &["iş günü"],
        conditions: &[
            "eğer",
            "şayet",
            "hariç",
            "haricinde",
            "ancak",
            "fakat",
            "istisna",
            "koşuluyla",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Nl,
        prohibited: "dit artikel kan niet worden verzonden\n\
            geen verzending voor dit artikel\n\
            dit artikel is alleen beschikbaar om af te halen\n\
            dit artikel kan alleen worden afgehaald",
        collection_only: "alleen afhalen\n\
            uitsluitend afhalen",
        worldwide: "dit artikel kan wereldwijd worden verzonden\n\
            wereldwijde verzending is beschikbaar voor dit artikel",
        on_request: "wereldwijde verzendkosten voor dit artikel op aanvraag\n\
            wereldwijde verzendkosten voor dit artikel zijn op aanvraag beschikbaar",
        calculated: "wereldwijde verzendkosten voor dit artikel worden bij het afrekenen berekend\n\
            wereldwijde verzendkosten voor dit artikel worden bij de kassa berekend",
        dispatch_prefix: r"verzending(?: van dit artikel)? binnen\s+",
        range_separator: "-|tot",
        dispatch_suffix: r"\s+werkdag(?:en)?",
        delivery_words: &[
            "verzend",
            "verzon",
            "bezorg",
            "lever",
            "gelever",
            "afhal",
            "afgehaald",
            "werkdag",
        ],
        delivery_markers: &[],
        conditions: &[
            "als",
            "indien",
            "tenzij",
            "behalve",
            "uitgezonderd",
            "mits",
            "maar",
            "echter",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Cs,
        prohibited: "toto zboží nelze odeslat\n\
            tuto položku nelze odeslat\n\
            toto zboží je dostupné pouze k osobnímu odběru\n\
            tato položka je dostupná pouze k osobnímu odběru",
        collection_only: "pouze osobní odběr\n\
            jen osobní odběr",
        worldwide: "toto zboží lze odeslat do celého světa\n\
            tuto položku lze odeslat do celého světa",
        on_request: "cena dopravy tohoto zboží do celého světa na vyžádání\n\
            náklady na dopravu tohoto zboží do celého světa na dotaz",
        calculated: "cena dopravy tohoto zboží do celého světa se vypočítá při placení\n\
            náklady na dopravu tohoto zboží do celého světa se vypočítají v pokladně",
        dispatch_prefix: r"(?:odeslání(?: tohoto zboží| této položky)? do|toto zboží odešleme do)\s+",
        range_separator: "-|až|az",
        dispatch_suffix: r"\s+(?:pracovních dnů|pracovního dne)",
        delivery_words: &[
            "odes", "neodes", "odeš", "doruč", "nedoruč", "doprav", "přeprav", "odběr", "odběru",
        ],
        delivery_markers: &[],
        conditions: &[
            "pokud",
            "jestliže",
            "ledaže",
            "kromě",
            "mimo",
            "ale",
            "avšak",
            "za podmínky",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Ja,
        prohibited: "この商品は発送できません\n\
            この商品は配送できません\n\
            この商品は店頭受け取りのみです\n\
            本商品は店頭引き取りのみです",
        collection_only: "店頭受け取りのみ\n\
            店頭引き取りのみ\n\
            引き取りのみ",
        worldwide: "この商品は世界各国へ発送できます\n\
            本商品は世界各国へ発送できます",
        on_request: "この商品の世界各国への送料はお問い合わせください\n\
            本商品の世界各国への送料はお問い合わせください",
        calculated: "この商品の世界各国への送料は決済時に計算されます\n\
            本商品の世界各国への送料はチェックアウト時に計算されます",
        dispatch_prefix: r"(?:この商品|本商品)は\s*",
        range_separator: "-|から",
        dispatch_suffix: r"\s*営業日(?:以内に|で)発送します",
        delivery_words: &[],
        delivery_markers: &[
            "発送",
            "配送",
            "送料",
            "受け取り",
            "引き取り",
            "お届け",
            "配達",
            "営業日",
        ],
        conditions: &[],
        condition_markers: &[
            "ただし",
            "但し",
            "場合",
            "除外",
            "例外",
            "状況により",
            "条件付き",
        ],
    },
    LanguagePhrases {
        language: Language::Ru,
        prohibited: "отправка этого товара недоступна\n\
            этот товар нельзя отправить\n\
            этот товар доступен только для самовывоза\n\
            для этого товара возможен только самовывоз",
        collection_only: "только самовывоз",
        worldwide: "этот товар можно отправить по всему миру\n\
            для этого товара доступна отправка по всему миру",
        on_request: "стоимость отправки этого товара по всему миру по запросу\n\
            стоимость доставки этого товара по всему миру по запросу",
        calculated: "стоимость отправки этого товара по всему миру рассчитывается при оформлении заказа\n\
            стоимость доставки этого товара по всему миру рассчитывается при оформлении заказа",
        dispatch_prefix: r"(?:отправка(?: этого товара)? в течение|этот товар будет отправлен в течение)\s+",
        range_separator: "-|до",
        dispatch_suffix: r"\s+(?:рабочих дней|рабочего дня)",
        delivery_words: &["отправ", "достав", "самовывоз", "пересыл", "рабочих"],
        delivery_markers: &[],
        conditions: &[
            "если",
            "кроме",
            "за исключением",
            "при условии",
            "однако",
            "но",
            "только если",
        ],
        condition_markers: &[],
    },
    LanguagePhrases {
        language: Language::Ar,
        prohibited: "الشحن غير متاح لهذا المنتج\n\
            لا يمكن شحن هذا المنتج\n\
            هذا المنتج متاح للاستلام الشخصي فقط\n\
            هذا المنتج متاح للاستلام من المتجر فقط",
        collection_only: "الاستلام الشخصي فقط\n\
            الاستلام من المتجر فقط",
        worldwide: "يمكن شحن هذا المنتج إلى جميع أنحاء العالم\n\
            الشحن إلى جميع أنحاء العالم متاح لهذا المنتج",
        on_request: "تكلفة شحن هذا المنتج إلى جميع أنحاء العالم عند الطلب\n\
            رسوم شحن هذا المنتج إلى جميع أنحاء العالم عند الطلب",
        calculated: "يتم حساب تكلفة شحن هذا المنتج إلى جميع أنحاء العالم عند إتمام الشراء\n\
            يتم حساب رسوم شحن هذا المنتج إلى جميع أنحاء العالم عند الدفع",
        dispatch_prefix: r"(?:يتم إرسال هذا المنتج خلال|إرسال هذا المنتج خلال|يُرسل هذا المنتج خلال)\s+",
        range_separator: "-|إلى|الى",
        dispatch_suffix: r"\s+(?:أيام عمل|يوم عمل)",
        delivery_words: &[
            "شحن",
            "الشحن",
            "إرسال",
            "الإرسال",
            "ارسال",
            "توصيل",
            "التوصيل",
            "تسليم",
            "التسليم",
            "استلام",
            "الاستلام",
        ],
        delivery_markers: &["شحن", "إرسال", "ارسال", "توصيل", "تسليم", "استلام"],
        conditions: &[
            "إذا",
            "اذا",
            "إلا",
            "الا",
            "باستثناء",
            "لكن",
            "ولكن",
            "بشرط",
            "ما لم",
        ],
        condition_markers: &["إذا", "إلا", "باستثناء", "بشرط"],
    },
];
