use super::LanguagePhrases;
use localization::Language;

pub(super) const ZH: LanguagePhrases = LanguagePhrases {
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
};

pub(super) const JA: LanguagePhrases = LanguagePhrases {
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
};
