use aws_tests_common::get_cfn_output;
use common::personalized::api::PersonalizedData;
use common::{
    batch::Batch,
    currency::{data::CurrencyData, domain::Currency},
    event::Event,
    event_id::EventId,
    has_key::HasKey,
    language::data::LanguageData,
    pagination::{cursor::api::TimeCursoredData, page::api::PaginatedData},
    price::domain::{FixedFxRate, FxRate, Price},
    product_id::{ProductKey, api::ProductKeyData},
    product_state::domain::ProductState,
    user_id::UserId,
};
use fake::{Fake, Faker};
use notification::{
    data::{
        get_notification_data::GetNotificationData, patch_notification_data::PatchNotificationData,
    },
    dynamodb::{
        notification_record::NotificationRecord,
        repository::{NotificationDynamoDbRepository, NotificationDynamoDbRepositoryImpl},
    },
};
use notification_api::notification_get::EventIdCursoredData;
use opensearch::GetParts;
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::product_record;
use product::{
    core::{
        authenticity::Authenticity,
        condition::Condition,
        origin_year::OriginYear,
        product_event::{
            ProductEvent, ProductEventPayload,
            domain::{
                ProductDomainEventPayload, ProductPriceChangeDomainEventPayload,
                ProductStateChangeDomainEventPayload,
            },
            enrichment::{EmbeddedProductEnrichmentEventPayload, ProductEnrichmentEventPayload},
            policy::{ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload},
        },
        product_image::ProductImage,
        prohibited_content::{ProhibitedContent, ProhibitedContentReason},
        provenance::Provenance,
        restoration::Restoration,
    },
    dynamodb::{
        authenticity_record::AuthenticityRecord,
        condition_record::ConditionRecord,
        product_event_record::ProductEventRecord,
        product_image_record::ProductImageRecord,
        product_record::{ProductRecord, mk_pk},
        product_state_record::ProductStateRecord,
        prohibited_content_record::ProhibitedContentRecord,
        provenance_record::ProvenanceRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
        restoration_record::RestorationRecord,
    },
    service::{
        command_service::{CommandProductService, CommandProductServiceImpl},
        product_command::{CreateProductCommand, UpdateProductCommand},
    },
};
use product_classification::category::service::MockCategoryService;
use product_classification::period::service::MockPeriodService;
use product_watchlist::dynamodb::repository::{
    WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
};
use product_watchlist_api::watchlist_patch::WatchlistProductPatch;
use search_filter::{
    core::user_search_filter_name::UserSearchFilterName,
    data::user_search_filter_data::UserSearchFilterData,
    dynamodb::repository::UserSearchFilterDynamoDbRepositoryImpl,
    opensearch::user_search_filter_document::UserSearchFilterDocument,
    service::user_search_filter_service::{UserSearchFilterService, UserSearchFilterServiceImpl},
};
use search_filter_api::{
    patch_types::{PatchProductSearchData, PatchUserSearchFilterData},
    post_types::PostUserSearchFilterData,
};
use serde::de::DeserializeOwned;
use shop::core::partner_shop_api_key::{HashedPartnerShopApiKey, PartnerShopApiKey};
use shop::data::get_shop_data::GetShopData;
use shop::dynamodb::repository::ShopDynamoDbRepository;
use shop::dynamodb::shop_record::ShopRecord;
use shop::{core::shop::Shop, dynamodb::repository::ShopDynamoDbRepositoryImpl};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};
use test_api::*;
use time::OffsetDateTime;
use user::core::tier::UserTier;
use user::dynamodb::tier_record::UserTierRecord;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::{
        repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
        user_record_update::UserRecordUpdate,
    },
};

// Shared 1024-dimensional text embedding used across multiple tests.
// Values are real embedding coordinates that produce meaningful ANN results in OpenSearch.
#[allow(dead_code)]
const EXAMPLE_EMBEDDING: [f32; 768] = [
    -0.036270842,
    0.02361682,
    -0.0029220004,
    -0.016072785,
    0.02316376,
    0.008332699,
    -0.02891746,
    0.015677461,
    -0.01463142,
    -0.10077077,
    0.029492525,
    0.02435133,
    0.04219972,
    -0.014070857,
    0.0025885715,
    0.015626293,
    -0.02128292,
    -0.016839612,
    -0.033849,
    -0.005133642,
    -0.015667764,
    -0.022695456,
    -0.0026581238,
    0.004976106,
    -0.06931419,
    -0.0021109623,
    -0.021948576,
    0.014820006,
    -0.013131463,
    0.15988831,
    0.0064275274,
    -0.0076653278,
    -0.038857676,
    0.015254312,
    -0.006424452,
    0.023108613,
    0.07357906,
    0.02665727,
    0.00575866,
    -0.0020714481,
    -0.025986703,
    0.027917072,
    -0.05469967,
    -0.021670582,
    -0.013154979,
    0.03821949,
    -0.012864586,
    0.0041407137,
    0.028950866,
    -0.0063043595,
    -0.008261838,
    0.020844104,
    0.00023263764,
    0.019758994,
    -0.019021928,
    0.03960655,
    -0.033878434,
    0.013370168,
    0.014440682,
    0.0015611759,
    -0.0060427976,
    -0.045798533,
    0.0028658975,
    0.0048241396,
    -0.026040733,
    0.02626537,
    0.019150974,
    -0.029956313,
    0.034417532,
    0.004912864,
    -0.010934778,
    0.0015013685,
    -0.022339396,
    0.020023942,
    0.005828301,
    -0.09966123,
    -0.06327092,
    0.024522135,
    -0.04826947,
    -0.020258049,
    -0.020873314,
    0.00036792032,
    -0.04074486,
    -0.019007195,
    0.0076569123,
    -0.0016037169,
    -0.014027866,
    0.0073729367,
    0.032381486,
    0.0052755023,
    0.0070434883,
    -0.012318134,
    -0.021978505,
    -0.0035620113,
    -0.035701845,
    -0.0062370175,
    -0.02363757,
    -0.03096813,
    0.00068176736,
    -0.012917327,
    0.0018843627,
    0.00052359427,
    -0.0044537387,
    -0.024308093,
    0.03562218,
    -0.011851221,
    0.028853856,
    -0.0012316285,
    0.02336089,
    0.0124050295,
    -0.03968709,
    -0.22498026,
    0.019794008,
    0.017281797,
    -0.003570257,
    0.25313136,
    -0.01618679,
    -0.014901762,
    -0.005371125,
    0.028242508,
    0.01495046,
    -0.002102732,
    -0.009359438,
    0.00038446576,
    0.038829945,
    0.03757913,
    0.061200988,
    0.039118737,
    -0.004323444,
    -0.027902763,
    0.021966223,
    0.036142662,
    0.0083741965,
    -0.014607301,
    0.013467545,
    0.015450331,
    -0.01713689,
    0.015013144,
    0.031145055,
    -0.03161453,
    -0.022872536,
    0.022965059,
    0.01465307,
    -0.040879726,
    -0.0070571224,
    0.0005238096,
    0.006517733,
    -0.05945249,
    -0.00067222246,
    -0.017303798,
    -0.02743768,
    0.051286776,
    0.010820717,
    -0.008597286,
    0.008311842,
    0.031794846,
    0.03725525,
    -0.007881769,
    0.034670442,
    -0.008120512,
    -0.0017984086,
    -0.008127016,
    -0.015096135,
    0.031332,
    0.013066103,
    -0.015996825,
    0.036567163,
    0.0023044932,
    -0.015515072,
    0.035640754,
    -0.025439778,
    0.019737234,
    -0.00048255606,
    0.027483864,
    -0.0062847566,
    0.035673726,
    0.02689843,
    -0.024476523,
    0.036291257,
    0.07619501,
    0.044448603,
    -0.02978229,
    0.0003071704,
    -0.066682085,
    -0.016464977,
    0.027141921,
    0.0015256412,
    -0.040789746,
    0.00044568328,
    -0.0073254695,
    0.020374568,
    0.009659304,
    0.021580324,
    0.00032814275,
    -0.033917915,
    -0.029009834,
    0.044985965,
    0.008687944,
    -0.040525082,
    0.01396069,
    -0.05742075,
    0.019486612,
    0.01334306,
    0.031041175,
    0.027065355,
    -0.012784972,
    0.0044180467,
    0.034939438,
    -0.013596606,
    0.020558216,
    0.011244942,
    -0.02307572,
    -0.019498749,
    -0.013778815,
    -0.0036768846,
    0.018824909,
    0.037605233,
    0.039746355,
    -0.0054461425,
    -0.01871201,
    -0.008835689,
    0.020823514,
    0.032042388,
    0.01331485,
    0.02537492,
    -0.0078030215,
    0.039240696,
    -0.021729227,
    -0.005688172,
    0.021090481,
    0.039646916,
    -0.034255978,
    -0.008763929,
    0.022813259,
    0.04913263,
    -0.008697633,
    -0.047809932,
    -0.0049542347,
    -0.000523725,
    0.00044063161,
    0.0046917875,
    0.0051231035,
    -0.04871753,
    0.010481537,
    0.001975782,
    -0.029364169,
    0.0010357029,
    0.030492049,
    -0.039915103,
    -0.008770563,
    0.027659342,
    -0.029857345,
    0.0154229775,
    0.0052343365,
    0.005864664,
    0.03145457,
    -0.041445766,
    0.014016001,
    -0.03302228,
    -0.013902694,
    -0.01625225,
    0.00993095,
    -0.01161224,
    -0.03400416,
    0.009857927,
    0.0104377465,
    0.060225435,
    -0.0093719335,
    0.0018534202,
    0.018284181,
    -0.01361248,
    0.017421937,
    -0.0038058027,
    0.042009708,
    -0.015804857,
    0.021955919,
    -0.0012992409,
    0.038149707,
    0.018156793,
    -0.062405195,
    0.013066391,
    -0.056466848,
    -0.017757474,
    -0.0028650656,
    0.0058570434,
    -0.010280581,
    0.021009846,
    0.016863098,
    -0.015731147,
    0.016432023,
    0.041244943,
    0.031222174,
    -0.0053466456,
    0.016777335,
    0.004303855,
    -0.0051430822,
    -0.01962097,
    0.00046041392,
    0.009175838,
    -0.008946787,
    -0.041479073,
    0.0012780037,
    0.01963695,
    -0.026783299,
    -0.01092655,
    0.03702143,
    0.012992049,
    0.008260065,
    -0.018874738,
    -0.01286012,
    0.016152298,
    -0.024768036,
    -0.024065694,
    0.0008564311,
    -0.003723401,
    -0.0047782045,
    0.012646516,
    0.011130584,
    0.007987915,
    -0.13179192,
    -0.018177606,
    0.02961083,
    0.010106819,
    0.008113584,
    -0.030036584,
    0.012636336,
    0.029913815,
    0.03315664,
    -0.008453596,
    -0.03339465,
    0.0021889387,
    0.013170344,
    -0.01902177,
    0.005910975,
    0.022003956,
    -0.0063015297,
    -0.0185965,
    0.0033527578,
    -0.022245914,
    -0.042567033,
    0.002801951,
    -0.17528647,
    0.0005035894,
    -0.017844167,
    -0.04551095,
    0.011306323,
    -0.030462844,
    0.0017954145,
    0.0061569316,
    0.019132044,
    0.029423045,
    0.023821782,
    0.018651243,
    0.062674895,
    0.008055076,
    0.027926216,
    0.0040267725,
    -0.0015232497,
    -0.010748787,
    -0.013262485,
    0.008980097,
    -0.033223867,
    0.0146368295,
    0.022167355,
    0.009057029,
    -0.023929827,
    -0.02951758,
    -0.0056341076,
    0.06293271,
    -0.017162772,
    0.026563834,
    0.055115834,
    0.03297112,
    0.044023864,
    0.03940343,
    0.030845787,
    -0.009692795,
    -0.00940617,
    -0.017781934,
    -0.0047045476,
    -0.017536366,
    -0.029622015,
    -0.026149537,
    0.014223205,
    0.042495977,
    -0.0290101,
    0.044529866,
    -0.0454436,
    -0.017035026,
    -0.043106273,
    0.004973654,
    0.29866093,
    -0.002671509,
    -0.035108592,
    -0.004368086,
    -0.037166778,
    -0.05845625,
    -0.0010122175,
    0.011301448,
    -0.035917412,
    -0.0042722896,
    0.0069688833,
    0.04308006,
    0.014895897,
    -0.00661524,
    -0.036040846,
    0.022869103,
    -0.004199664,
    -0.010235386,
    0.0077593494,
    -0.0121860765,
    -0.046512168,
    -0.0064643933,
    -0.0047807526,
    -0.018116102,
    0.023745356,
    -0.040249992,
    -0.031160146,
    -0.05771907,
    -0.02815563,
    -0.0068371277,
    -0.01035654,
    0.024611121,
    -0.007522822,
    0.017330028,
    0.022064786,
    0.011030672,
    -0.011998312,
    -0.0041401656,
    -0.0062133586,
    -0.04972406,
    -0.011494944,
    -0.0047495724,
    0.018067274,
    0.039112672,
    -0.019449852,
    0.0065324428,
    -0.02769223,
    -0.039807513,
    0.006461706,
    0.035815254,
    0.0017134275,
    -0.005184694,
    -0.022443162,
    -0.0072568725,
    -0.002618277,
    0.015006618,
    -0.007317327,
    0.037664324,
    -0.023994833,
    0.0054134326,
    -0.003410414,
    -0.0237863,
    0.01482158,
    -0.014767443,
    -0.015756682,
    -0.0022374734,
    0.026522176,
    0.0030798607,
    -0.012200735,
    -0.0686059,
    -0.01256213,
    0.01759631,
    0.0014242876,
    0.044622954,
    0.028350726,
    -0.008226041,
    0.015207355,
    0.0146250725,
    0.015122039,
    -0.03984472,
    -0.02007866,
    0.0028963448,
    0.039672844,
    -0.057417013,
    0.048817653,
    -0.02627826,
    0.0134779485,
    -0.008799786,
    -0.0030325444,
    -0.012617669,
    -0.00087181904,
    0.019178504,
    0.011707547,
    -0.0065853586,
    -0.008898021,
    0.015297573,
    -0.04113959,
    0.01135404,
    -0.018460345,
    0.005675249,
    -0.02876876,
    -0.0065206215,
    0.006008467,
    0.04377509,
    -0.016163269,
    -0.009146873,
    -0.0015525562,
    0.0007020318,
    -0.02461698,
    -0.0344008,
    0.012333875,
    -0.011139719,
    0.011816653,
    -0.014555361,
    -0.0003070767,
    -0.00907902,
    -0.19088055,
    0.015713643,
    0.037807066,
    -0.019069457,
    -0.008042357,
    0.049934104,
    -0.021369996,
    0.0140267825,
    0.00420878,
    0.007308135,
    -0.028600363,
    0.016940795,
    -0.05842496,
    0.006888315,
    0.065117255,
    0.020332089,
    0.014443868,
    -0.065477155,
    0.0010837859,
    -0.005974733,
    0.007969608,
    -0.07594507,
    0.0029710634,
    0.010829651,
    -0.0012731664,
    0.0017792372,
    -0.014663885,
    -0.0203348,
    0.016117094,
    -0.03351677,
    -0.031653583,
    0.0020854105,
    -0.036179002,
    0.0034623882,
    0.010883555,
    0.029086262,
    -0.037473448,
    0.02590499,
    -0.008166385,
    0.009189521,
    0.020489529,
    0.038782965,
    0.029644571,
    -0.0018531352,
    0.047954768,
    -0.014560271,
    0.03497629,
    -0.2864895,
    0.030249074,
    -0.008526756,
    -0.03771894,
    -0.03704407,
    -0.056556262,
    -0.030370766,
    -0.015169972,
    0.03480281,
    0.006294808,
    -0.0067806765,
    0.011883565,
    -0.026535155,
    0.026770437,
    -0.040663313,
    0.005396514,
    -0.0063958433,
    -0.0102125285,
    0.040829312,
    0.02465255,
    0.050618887,
    -0.02336513,
    -0.01293364,
    -0.004051999,
    0.021325089,
    -0.056525428,
    0.008540481,
    0.017834686,
    -0.022880128,
    0.005065879,
    -0.023469102,
    0.024000825,
    0.028049674,
    0.01294549,
    0.026906919,
    0.0038596892,
    -0.018538222,
    -0.01048302,
    0.068679444,
    0.020244043,
    0.018993724,
    0.019902077,
    0.017294884,
    0.010387368,
    -0.013022128,
    -0.007912021,
    0.016969386,
    -0.0005016011,
    0.014465001,
    -0.0020284972,
    0.0066795074,
    0.0014131917,
    -0.039595082,
    -0.037901003,
    -0.0056755184,
    -0.0134587595,
    -0.019023392,
    0.02653589,
    -0.010009279,
    0.012755573,
    0.021138454,
    0.024111101,
    -0.0049227146,
    -0.021821024,
    -0.0038105084,
    0.00024833335,
    0.0015837409,
    0.011216936,
    -0.0011316041,
    0.040861413,
    0.030079305,
    0.020069895,
    0.018964952,
    0.025762206,
    -0.027975056,
    0.006083612,
    0.041216183,
    -0.0198914,
    -0.037045345,
    0.009628558,
    -0.004648141,
    -0.023070302,
    -0.025827674,
    0.032872,
    0.05590265,
    -0.0074252035,
    -0.02661827,
    -0.018210603,
    0.0076413676,
    0.026913233,
    0.014321531,
    0.0049917623,
    0.029138755,
    -0.00072933227,
    0.012737821,
    -0.011692181,
    0.01370206,
    -0.019096408,
    -0.017844934,
    -0.036554847,
    0.046650723,
    -0.01349189,
    -0.02140371,
    -0.016438346,
    -0.013416906,
    0.0006781695,
    0.060341988,
    -0.020184021,
    0.006736895,
    -0.005342232,
    -0.0012715309,
    -0.023459038,
    0.021250091,
    -0.01638936,
    0.009222685,
    0.017368332,
    0.005119245,
    -0.014245158,
    0.070186354,
    -0.0136648305,
    0.015072559,
    0.011200258,
    0.0020309482,
    -0.011483067,
    0.032985736,
    0.040143743,
    -0.02111509,
    0.009001113,
    -0.016965754,
    -0.0035368428,
    -0.03147873,
    0.038750287,
    -0.025312606,
    -0.010575393,
    -0.0041843024,
    -0.025241813,
    -0.02481768,
    0.0054268795,
    -0.013957707,
    -0.005630476,
    0.016301252,
    -0.009564899,
    0.040901873,
    0.0077344235,
    -0.034312457,
    0.0070438446,
    0.06272498,
    0.04281858,
    -0.012747501,
    0.057406064,
    0.03071193,
    -0.00536834,
    -0.03017276,
    -0.019532941,
    -0.0067724953,
    -0.0092885615,
    0.042543396,
    -0.056247883,
    -0.026811935,
    0.020648511,
    -0.04053965,
    0.009476278,
    0.0073615,
    0.009893717,
    0.012109694,
    0.022592265,
    -0.060574617,
    0.010043578,
    0.016987907,
    0.03377897,
    -0.030498004,
    -0.01750739,
    0.067946605,
    0.007580749,
    0.0012963444,
    0.019019201,
    -0.0069742682,
    -0.011390442,
    0.29936674,
    0.03025758,
    -0.033098254,
    -0.020426897,
    0.019291064,
    -0.022534057,
    0.016344719,
    -0.023486739,
    0.018488541,
    -0.048147343,
    -0.010020716,
    -0.040037777,
    -0.03153086,
    -0.054456603,
    0.0065903524,
    0.024521971,
    -0.060160343,
    0.0045516137,
    0.013521374,
    -0.010743019,
    0.008624498,
    0.04089224,
    -0.021292338,
    0.002317146,
    0.02656671,
    0.024010226,
    0.020137409,
    -0.030693509,
    -0.060116753,
    0.024448955,
    0.015258921,
    -0.04637649,
    0.013759733,
    0.0059404382,
    -0.006709233,
    0.019880014,
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    dynamodb_repository
        .put_shop_record(shop.clone().into())
        .await
        .unwrap();
    shop
}

async fn create_products(commands: Vec<CreateProductCommand>) {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let fx_rate = FixedFxRate();
    let mut period_service = MockPeriodService::default();
    period_service
        .expect_find_periods()
        .returning(|| Box::pin(async { Ok(vec![]) }));
    let mut category_service = MockCategoryService::default();
    category_service
        .expect_find_categories()
        .returning(|| Box::pin(async { Ok(vec![]) }));
    let command_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        &period_service,
        &category_service,
    );

    let result = command_service.create(commands).await;
    assert!(result.is_empty(), "Some products failed to create");
}

async fn update_products(commands: HashMap<ProductKey, UpdateProductCommand>) {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let fx_rate = FixedFxRate();
    let mut period_service = MockPeriodService::default();
    period_service
        .expect_find_periods()
        .returning(|| Box::pin(async { Ok(vec![]) }));
    let mut category_service = MockCategoryService::default();
    category_service
        .expect_find_categories()
        .returning(|| Box::pin(async { Ok(vec![]) }));
    let command_service = CommandProductServiceImpl::new(
        &product_repository,
        &fx_rate,
        &period_service,
        &category_service,
    );

    let result = command_service.update(commands).await;
    assert!(result.is_empty(), "Some products failed to update");
}

/// Polls OpenSearch until a document with the given `id` appears in `index`, issuing an explicit
/// index refresh before each attempt. This is necessary because Localstack's OpenSearch requires
/// a refresh before documents become visible — even via direct GET by ID.
#[allow(dead_code)]
async fn wait_for_document<T: DeserializeOwned>(index: &'static str, id: impl Into<String>) -> T {
    let id = id.into();
    for _ in 0..24 {
        refresh_index(index).await;
        if let Some(doc) = try_read_by_id::<T>(index, &id).await {
            return doc;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!(
        "Expected document '{}' in index '{}' but it never appeared after 120s",
        id, index
    );
}

#[allow(dead_code)]
async fn try_read_by_id<T: DeserializeOwned>(index: &str, id: impl Into<String>) -> Option<T> {
    let get_response = get_opensearch_client()
        .await
        .get(GetParts::IndexId(index, &id.into()))
        .send()
        .await
        .unwrap();
    if get_response.status_code().as_u16() == 404 {
        return None;
    }
    let get_response = get_response.error_for_status_code().unwrap();
    let response_doc: serde_json::Value = get_response.json().await.unwrap();
    Some(serde_json::from_value(response_doc["_source"].clone()).unwrap())
}

#[allow(dead_code)]
async fn wait_until_document_exists(
    user_search_filter_id: impl Into<String>,
) -> UserSearchFilterDocument {
    let user_search_filter_id = user_search_filter_id.into();
    for _ in 0..24 {
        refresh_index("user_search_filters").await;
        if let Some(document) = try_read_by_id::<UserSearchFilterDocument>(
            "user_search_filters",
            &user_search_filter_id,
        )
        .await
        {
            return document;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!(
        "Expected search-filter document '{}' to exist in OpenSearch, but it did not appear in time.",
        user_search_filter_id
    );
}

#[allow(dead_code)]
async fn wait_until_document_deleted(user_search_filter_id: impl Into<String>) {
    let user_search_filter_id = user_search_filter_id.into();
    for _ in 0..24 {
        refresh_index("user_search_filters").await;
        if try_read_by_id::<UserSearchFilterDocument>("user_search_filters", &user_search_filter_id)
            .await
            .is_none()
        {
            return;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!(
        "Expected search-filter document '{}' to be deleted from OpenSearch, but it still existed.",
        user_search_filter_id
    );
}

// ---------------------------------------------------------------------------
// Product ingest: DynamoDB materialization
// Verifies EventBridge routing and Lambda IAM access for each event type.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_when_put_new_item() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let mut create_cmd: CreateProductCommand = Faker.fake();
    create_cmd.shop_id = shop.shop_id;
    create_cmd.shop_name = shop.name.clone();
    create_cmd.shop_type = shop.shop_type;

    create_products(vec![create_cmd.clone()]).await;

    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(create_cmd.shops_product_id, materialized.shops_product_id);
            assert_eq!(create_cmd.url, materialized.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not found in DynamoDB after 60s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_domain_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_state: ProductState = match materialized_old.state {
        ProductStateRecord::Available => ProductState::Sold,
        _ => ProductState::Available,
    };
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: None,
        state: Some(new_state),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && new_state == ProductState::from(materialized.state)
        {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(new_state, ProductState::from(materialized.state));
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with expected state after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_estimate_price_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.price_estimate_min_native = None;
    materialized_old.price_estimate_max_native = None;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_min_price = Price::new(1000u64.into(), Currency::Eur);
    let new_max_price = Price::new(5000u64.into(), Currency::Eur);
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: Some(new_min_price),
        native_price_estimate_max: Some(new_max_price),
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.price_estimate_min_native.is_some()
        {
            assert_eq!(
                new_min_price,
                materialized.price_estimate_min_native.unwrap().into()
            );
            assert_eq!(
                new_max_price,
                materialized.price_estimate_max_native.unwrap().into()
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with estimate prices after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_url_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    let domain = shop.domains.into_iter().next().unwrap();
    materialized_old
        .url
        .set_host(Some(domain.as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_url = url::Url::parse(&format!("https://{}/new-product-url", domain)).unwrap();
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: Some(new_url.clone()),
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.url == new_url
        {
            assert_eq!(new_url, materialized.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with new url after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_images_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.images = vec![];
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_images: Vec<ProductImage> = fake::vec![ProductImage; 2];
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: Some(new_images.clone()),
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.images.len() == 2
        {
            let materialized_images: Vec<ProductImage> =
                materialized.images.into_iter().map(|i| i.into()).collect();
            assert_eq!(new_images, materialized_images);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with new images after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_auction_time_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.auction_start = None;
    materialized_old.auction_end = None;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_start = OffsetDateTime::now_utc();
    let new_end = new_start + time::Duration::days(7);
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: Some(new_start),
        auction_end: Some(new_end),
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.auction_start.is_some()
        {
            assert!(materialized.auction_start.is_some());
            assert!(materialized.auction_end.is_some());
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with auction times after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_origin_year_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.origin_year = None;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let new_origin_year = OriginYear::ExactYear(1850i32.into());
    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: Some(new_origin_year),
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.origin_year.is_some()
        {
            assert_eq!(
                Some(common::year::Year::from(1850i32)),
                materialized.origin_year
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with origin year after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_authenticity_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.authenticity = AuthenticityRecord::Unknown;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: Some(Authenticity::Original),
        condition: None,
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.authenticity == AuthenticityRecord::Original
        {
            assert_eq!(AuthenticityRecord::Original, materialized.authenticity);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with authenticity after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_condition_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.condition = ConditionRecord::Unknown;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: Some(Condition::Excellent),
        provenance: None,
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.condition == ConditionRecord::Excellent
        {
            assert_eq!(ConditionRecord::Excellent, materialized.condition);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with condition after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_provenance_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.provenance = ProvenanceRecord::Unknown;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: Some(Provenance::Complete),
        restoration: None,
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.provenance == ProvenanceRecord::Complete
        {
            assert_eq!(ProvenanceRecord::Complete, materialized.provenance);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with provenance after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_restoration_changed_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.restoration = RestorationRecord::Unknown;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_key = ProductKey {
        shop_id: shop.shop_id,
        shops_product_id: materialized_old.shops_product_id.clone(),
    };
    let update_cmd = UpdateProductCommand {
        native_price: materialized_old.price_native.map(|p| p.into()),
        state: Some(materialized_old.state.into()),
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        origin_year: None,
        authenticity: None,
        condition: None,
        provenance: None,
        restoration: Some(Restoration::Major),
    };

    update_products(HashMap::from([(product_key.clone(), update_cmd)])).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized.restoration == RestorationRecord::Major
        {
            assert_eq!(RestorationRecord::Major, materialized.restoration);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with restoration after 60s",
                shop.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_enrichment_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.embedding = None;
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let embedding = vec![0.4269f32; 768];
    let product_event_records = Batch::try_from_iter([ProductEventRecord::from(ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::Embedded(EmbeddedProductEnrichmentEventPayload {
                shop_id: materialized_old.shop_id,
                seller_id: materialized_old.seller_id,
                shops_product_id: materialized_old.shops_product_id.clone(),
                embedding: embedding.clone(),
            }),
        ),
    })])
    .unwrap();
    repository
        .put_product_event_records(product_event_records)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && let Some(actual_embedding) = materialized.embedding
        {
            assert_eq!(embedding, actual_embedding);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with embedding after 60s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_dynamodb_for_policy_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    materialized_old.images = fake::vec![ProductImageRecord; 3]
        .into_iter()
        .map(|mut img| {
            img.prohibited_content = ProhibitedContentRecord::Unknown;
            img
        })
        .collect();
    let insert_res = repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_event_records = Batch::try_from_iter([ProductEventRecord::from(ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductPolicyEvent(
            ProductPolicyEventPayload::ProhibitedContentDecision(
                ProhibitedContentProductPolicyEventPayload {
                    shop_id: materialized_old.shop_id,
                    seller_id: materialized_old.seller_id,
                    shops_product_id: materialized_old.shops_product_id.clone(),
                    decision: ProhibitedContent::NaziGermany,
                    reason: ProhibitedContentReason::ProductText,
                },
            ),
        ),
    })])
    .unwrap();
    repository
        .put_product_event_records(product_event_records)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &materialized_old.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && materialized
                .images
                .iter()
                .any(|img| img.prohibited_content == ProhibitedContentRecord::NaziGermany)
        {
            assert!(
                materialized
                    .images
                    .iter()
                    .all(|img| img.prohibited_content == ProhibitedContentRecord::NaziGermany)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with policy decision after 60s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ---------------------------------------------------------------------------
// Product ingest: OpenSearch materialization
// Verifies EventBridge routing and Lambda IAM access for each event type.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_opensearch_for_create_product_command() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data.title = LocalizedTextData {
        text: "Exactly the expected title".to_string(),
        language: LanguageData::En,
    };
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();

    upsert_products(vec![put_product_data.clone()]).await;

    let os_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        refresh_index("products").await;
        let hit = os_repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Eur,
                    product_query: Some("Exactly the expected title".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    auction_start_query: None,
                    auction_end_query: None,
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .into_iter()
            .next();

        if let Some(hit) = hit {
            assert_eq!(shop.shop_id, hit.source.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                hit.source.shops_product_id
            );
            assert_eq!(put_product_data.url, hit.source.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument for shop '{}' / product '{}' not found in OpenSearch after 60s",
                shop.shop_id, put_product_data.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_opensearch_for_domain_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let ddb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let os_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.title_en = Some("Exactly the expected title".to_string());
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = ddb_repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let os_doc: ProductDocument = materialized_old.clone().into();
    let insert_res = os_repository
        .create_product_documents(vec![os_doc])
        .await
        .unwrap();
    assert!(!insert_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let new_state = match materialized_old.state {
        ProductStateRecord::Available => ProductStateData::Sold,
        _ => ProductStateData::Available,
    };
    let put_product_data = PutProductData {
        shops_product_id: materialized_old.shops_product_id,
        title: Faker.fake(),
        description: None,
        price: None,
        price_estimate_min: Faker.fake(),
        price_estimate_max: Faker.fake(),
        state: new_state,
        url: materialized_old.url,
        images: materialized_old.images.into_iter().map(|i| i.url).collect(),
        auction_start: None,
        auction_end: None,
    };

    upsert_products(vec![put_product_data.clone()]).await;

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        refresh_index("products").await;
        let hit = os_repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Usd,
                    product_query: Some("Exactly the expected title".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    auction_start_query: None,
                    auction_end_query: None,
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .into_iter()
            .next();

        if let Some(hit) = hit
            && ProductState::from(new_state) == ProductState::from(hit.source.state)
        {
            assert_eq!(shop.shop_id, hit.source.shop_id);
            assert_eq!(
                ProductState::from(new_state),
                ProductState::from(hit.source.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument for shop '{}' / product '{}' not updated with expected state after 60s",
                shop.shop_id, put_product_data.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_opensearch_for_enrichment_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let ddb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let os_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.embedding = None;
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.title_en = Some("Exactly the expected title".to_string());
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = ddb_repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let mut os_doc: ProductDocument = materialized_old.clone().into();
    os_doc.embedding = None;
    let insert_res = os_repository
        .create_product_documents(vec![os_doc])
        .await
        .unwrap();
    assert!(!insert_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let product_event_records = Batch::try_from_iter([ProductEventRecord::from(ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::Embedded(
                EmbeddedProductEnrichmentEventPayload {
                    shop_id: materialized_old.shop_id,
                    shops_product_id: materialized_old.shops_product_id.clone(),
                    embedding: EXAMPLE_EMBEDDING.into(),
                },
            ),
        ),
    })])
    .unwrap();
    ddb_repository
        .put_product_event_records(product_event_records)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        refresh_index("products").await;
        let hit = os_repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Usd,
                    product_query: Some("Exactly the expected title".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    auction_start_query: None,
                    auction_end_query: None,
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .into_iter()
            .next();

        if let Some(hit) = hit
            && let Some(embedding) = hit.source.embedding
        {
            assert_eq!(EXAMPLE_EMBEDDING.as_slice(), &embedding);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument for shop '{}' / product '{}' not updated with embedding after 120s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_materialize_product_in_opensearch_for_policy_event() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let ddb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let os_repository = ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut materialized_old: ProductRecord = Faker.fake();
    materialized_old.images = fake::vec![ProductImageRecord; 3]
        .into_iter()
        .map(|mut img| {
            img.prohibited_content = ProhibitedContentRecord::Unknown;
            img
        })
        .collect();
    materialized_old.pk = mk_pk(&shop.shop_id, &materialized_old.shops_product_id);
    materialized_old.shop_id = shop.shop_id;
    materialized_old.title_en = Some("Exactly the expected title".to_string());
    materialized_old
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let insert_res = ddb_repository
        .put_product_records([materialized_old.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap_or_default().is_empty());

    let os_doc: ProductDocument = materialized_old.clone().into();
    let insert_res = os_repository
        .create_product_documents(vec![os_doc])
        .await
        .unwrap();
    assert!(!insert_res.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let product_event_records = Batch::try_from_iter([ProductEventRecord::from(ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductPolicyEvent(
            ProductPolicyEventPayload::ProhibitedContentDecision(
                ProhibitedContentProductPolicyEventPayload {
                    shop_id: materialized_old.shop_id,
                    shops_product_id: materialized_old.shops_product_id.clone(),
                    decision: ProhibitedContent::NaziGermany,
                    reason: ProhibitedContentReason::ProductText,
                },
            ),
        ),
    })])
    .unwrap();
    ddb_repository
        .put_product_event_records(product_event_records)
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        refresh_index("products").await;
        let hit = os_repository
            .search_product_documents(
                &ProductSearch {
                    language: common::language::domain::Language::En,
                    currency: common::currency::domain::Currency::Usd,
                    product_query: Some("Exactly the expected title".try_into().unwrap()),
                    category_id: Default::default(),
                    period_id: Default::default(),
                    shop_name_query: Default::default(),
                    exclude_shop_name_query: Default::default(),
                    seller_name_query: Default::default(),
                    exclude_seller_name_query: Default::default(),
                    shop_type_query: Default::default(),
                    price_query: None,
                    state_query: Default::default(),
                    origin_year_query: None,
                    authenticity_query: Default::default(),
                    condition_query: Default::default(),
                    provenance_query: Default::default(),
                    restoration_query: Default::default(),
                    auction_start_query: None,
                    auction_end_query: None,
                    created_query: None,
                    updated_query: None,
                },
                &Sort {
                    sort: SortProductField::Score,
                    order: SortOrder::Desc,
                },
                &None,
            )
            .await
            .unwrap()
            .hits
            .hits
            .into_iter()
            .next();

        if let Some(hit) = hit
            && hit
                .source
                .images
                .iter()
                .any(|img| img.prohibited_content == ProhibitedContentDocument::NaziGermany)
        {
            assert!(
                hit.source
                    .images
                    .iter()
                    .all(|img| img.prohibited_content == ProhibitedContentDocument::NaziGermany)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductDocument for shop '{}' / product '{}' not updated with policy decision after 60s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
*/

// ---------------------------------------------------------------------------
// User account
// Verifies the Cognito post-confirmation Lambda trigger writes to DynamoDB,
// and that the user API enforces Cognito auth (IAM policy).
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_create_dynamodb_user_record_on_signup() {
    let cfn = get_cfn_output();
    let cognito = get_cognito_client().await;

    let email: String = fake::faker::internet::de_de::SafeEmail().fake();
    let password: String = format!(
        "{}*1bC",
        fake::faker::internet::de_de::Password(8..12).fake::<String>()
    );

    let user_id: UserId = cognito
        .sign_up()
        .client_id(&cfn.cognito_user_pool_client_public_id)
        .username(&email)
        .password(password)
        .user_attributes(
            aws_sdk_cognitoidentityprovider::types::AttributeType::builder()
                .name("email")
                .value(&email)
                .build()
                .unwrap(),
        )
        .send()
        .await
        .unwrap()
        .user_sub
        .try_into()
        .unwrap();
    cognito
        .admin_confirm_sign_up()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(&email)
        .send()
        .await
        .unwrap();

    tokio::time::sleep(Duration::from_secs(5)).await;

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let res = user_repository.get_user_record(&user_id).await.unwrap();
    assert!(res.is_some_and(|r| r.email == email));
}

#[localstack_test(services = [Cloudformation()])]
async fn should_get_and_patch_user_account() {
    let user = create_random_test_user().await;
    let url = format!(
        "{}/api/v1/me/account",
        get_cfn_output().api_gateway_endpoint_url,
    );

    let get_response1 = reqwest::Client::new()
        .get(url.clone())
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response1.status());
    let gotten1 = get_response1.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(UserId::from(user.sub), gotten1.user_id);

    let patch_data = PatchUserAccountData {
        first_name: Some("Hansi".into()),
        last_name: Some("Hans".into()),
        language: Some(LanguageData::Fr),
        currency: Some(CurrencyData::Nzd),
        prohibited_content_consent: None,
    };
    let patch_response = reqwest::Client::new()
        .patch(url.clone())
        .bearer_auth(user.access_token.clone())
        .json(&patch_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(UserId::from(user.sub), patched.user_id);
    assert_eq!(
        &patch_data.first_name.unwrap(),
        patched.first_name.as_ref().unwrap()
    );
    assert_eq!(
        &patch_data.last_name.unwrap(),
        patched.last_name.as_ref().unwrap()
    );
    assert_eq!(
        &patch_data.language.unwrap(),
        patched.language.as_ref().unwrap()
    );
    assert_eq!(
        &patch_data.currency.unwrap(),
        patched.currency.as_ref().unwrap()
    );

    let get_response2 = reqwest::Client::new()
        .get(url.clone())
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response2.status());
    let gotten2 = get_response2.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(patched, gotten2);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_delete_user_from_cognito_and_dynamodb() {
    let cfn = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    // Verify user record exists in DynamoDB
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let record_before = user_repository.get_user_record(&user_id).await.unwrap();
    assert!(
        record_before.is_some(),
        "User record should exist before deletion"
    );

    // Call DELETE /api/v1/me via API Gateway
    let delete_url = format!("{}/api/v1/me", cfn.api_gateway_endpoint_url);
    let delete_response = reqwest::Client::new()
        .delete(&delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status().as_u16());

    // Verify user record is deleted from DynamoDB
    let record_after = user_repository.get_user_record(&user_id).await.unwrap();
    assert!(
        record_after.is_none(),
        "User record should be deleted from DynamoDB"
    );

    // Verify user is deleted from Cognito
    let cognito = get_cognito_client().await;
    let cognito_user = cognito
        .admin_get_user()
        .user_pool_id(&cfn.cognito_user_pool_id)
        .username(user.sub.to_string())
        .send()
        .await;
    assert!(
        cognito_user.is_err(),
        "Cognito user should no longer exist after deletion"
    );

    // Verify the deleted user's access token no longer works
    let get_url = format!("{}/api/v1/me/account", cfn.api_gateway_endpoint_url);
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_ne!(
        200,
        get_response.status().as_u16(),
        "Deleted user should not be able to access their account"
    );
}

// ---------------------------------------------------------------------------
// Product update → notify user
// Verifies EventBridge → SQS → Lambda → Cognito/DynamoDB → SES routing
// and the associated IAM policies.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_send_email_to_user_when_watched_product_has_update() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // Create product
    let mut create_cmd: CreateProductCommand = Faker.fake();
    create_cmd.shop_id = shop.shop_id;
    create_cmd.shop_name = shop.name.clone();
    create_cmd.shop_type = shop.shop_type;
    create_products(vec![create_cmd.clone()]).await;
    tokio::time::sleep(Duration::from_secs(45)).await;

    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    assert!(
        product_repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap()
            .is_some()
    );

    // Create and configure user
    let user = create_test_user("watchlist-test@example.com").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    assert!(
        user_repository
            .get_user_record(&user.sub.into())
            .await
            .unwrap()
            .is_some()
    );
    user_repository
        .update_user_record(
            &user.sub.into(),
            UserRecordUpdate {
                first_name: Some("Thomas".into()),
                last_name: Some("Testperson".into()),
                language: Some(common::language::record::LanguageRecord::De),
                currency: Some(common::currency::record::CurrencyRecord::Eur),
                prohibited_content_consent: None,
                tier: Some(UserTierRecord::Free),
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();

    // Add product to watchlist
    let post_url = format!("{}/api/v1/me/watchlist", stack.api_gateway_endpoint_url);
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ProductKeyData {
            shop_id: shop.shop_id,
            shops_product_id: create_cmd.shops_product_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Enable notifications
    let patch_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        stack.api_gateway_endpoint_url, shop.shop_id, create_cmd.shops_product_id
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&WatchlistProductPatch {
            notifications: Some(true),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response
        .json::<PersonalizedData<GetProductData, ProductUserStateData>>()
        .await
        .unwrap();
    tokio::time::sleep(Duration::from_secs(10)).await;

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let eligible = watchlist_repository
        .query_user_ids_watching_product(&patched.item.product_id)
        .await
        .unwrap();
    let eligible_user_ids: Vec<UserId> = eligible.into_iter().map(|(user_id, _)| user_id).collect();
    assert_eq!(vec![UserId::from(user.sub)], eligible_user_ids);
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Update product state to trigger notification
    let new_state = if matches!(create_cmd.state, ProductState::Available) {
        ProductState::Sold
    } else {
        ProductState::Available
    };
    update_products(HashMap::from([(
        create_cmd.key(),
        UpdateProductCommand {
            native_price: None,
            state: Some(new_state),
            native_price_estimate_min: None,
            native_price_estimate_max: None,
            url: None,
            images: None,
            auction_start: None,
            auction_end: None,
            origin_year: None,
            authenticity: None,
            condition: None,
            provenance: None,
            restoration: None,
        },
    )]))
    .await;

    assert!(wait_for_ses_email("Statusänderung", Duration::from_secs(120)).await);
}

// ---------------------------------------------------------------------------
// Search filter: OpenSearch sync
// Verifies EventBridge → SQS → Lambda → OpenSearch routing and IAM access
// for create, update, and delete operations on user search filters.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_create_search_filter_and_sync_it_to_opensearch() {
    let user = create_random_test_user().await;

    let expected = PostUserSearchFilterData {
        name: "Staging sync create".into(),
        search: ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Barock Kommode".try_into().unwrap()),
            category_id: HashSet::from_iter([CategoryId::from("furniture")]),
            period_id: HashSet::from_iter([PeriodId::from("baroque")]),
            shop_name_query: HashSet::from_iter([ShopName::from("Galerie Test")]),
            exclude_shop_name_query: HashSet::from_iter([ShopName::from("Do Not Match Shop")]),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
            price_query: Some(RangeQuery {
                min: Some(100),
                max: Some(5000),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            origin_year_query: Some(RangeQuery {
                min: Some(1700.into()),
                max: Some(1800.into()),
            }),
            authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
            condition_query: HashSet::from_iter([ConditionData::Excellent]),
            provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
            restoration_query: HashSet::from_iter([RestorationData::Minor]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2020-01-01 0:00 UTC)),
                max: Some(datetime!(2030-01-01 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2020-01-01 0:00 UTC)),
                max: Some(datetime!(2030-01-01 0:00 UTC)),
            }),
            auction_start_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2026-01-01 0:00 UTC)),
            }),
            auction_end_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2026-01-01 0:00 UTC)),
            }),
        },
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(expected.name, posted.name);
    assert_eq!(expected.search, posted.search);
    assert_eq!(user.sub.to_string(), posted.user_id.to_string());

    let document = wait_until_document_exists(posted.user_search_filter_id.to_string()).await;
    assert_eq!(posted.user_search_filter_id, document.user_search_filter_id);
    assert_eq!(posted.user_id, document.user_id);
    assert_eq!(posted.name, document.name);
    assert_eq!(posted.created, document.created);
    assert_eq!(posted.updated, document.updated);
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_update_search_filter_and_sync_changes_to_opensearch() {
    let user = create_random_test_user().await;

    let initial = PostUserSearchFilterData {
        name: "Staging sync update initial".into(),
        search: ProductSearchData {
            language: LanguageData::De,
            currency: CurrencyData::Eur,
            product_query: Some("Barock".try_into().unwrap()),
            category_id: HashSet::from_iter([CategoryId::from("furniture")]),
            period_id: HashSet::from_iter([PeriodId::from("baroque")]),
            shop_name_query: HashSet::from_iter([ShopName::from("Initial Shop")]),
            exclude_shop_name_query: HashSet::from_iter([ShopName::from("Initial Excluded Shop")]),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
            price_query: Some(RangeQuery {
                min: Some(50),
                max: Some(1000),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            origin_year_query: Some(RangeQuery {
                min: Some(1680.into()),
                max: Some(1780.into()),
            }),
            authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
            condition_query: HashSet::from_iter([ConditionData::Good]),
            provenance_query: HashSet::from_iter([ProvenanceData::Partial]),
            restoration_query: HashSet::from_iter([RestorationData::None]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2021-01-01 0:00 UTC)),
                max: Some(datetime!(2031-01-01 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2021-01-01 0:00 UTC)),
                max: Some(datetime!(2031-01-01 0:00 UTC)),
            }),
            auction_start_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2025-01-01 0:00 UTC)),
            }),
            auction_end_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2025-01-01 0:00 UTC)),
            }),
        },
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&initial)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    let initial_document =
        wait_until_document_exists(posted.user_search_filter_id.to_string()).await;
    assert_eq!(posted.name, initial_document.name);

    let patch = PatchUserSearchFilterData {
        name: Some("Staging sync update patched".into()),
        notifications: None,
        search: Some(PatchProductSearchData {
            language: Some(LanguageData::Fr),
            currency: Some(CurrencyData::Usd),
            product_query: Some("Louis XV".try_into().unwrap()),
            category_id: Some(HashSet::from_iter([CategoryId::from("decorative-objects")])),
            period_id: Some(HashSet::from_iter([PeriodId::from("rococo")])),
            shop_name_query: Some(HashSet::from_iter([ShopName::from("Patched Shop")])),
            exclude_shop_name_query: None,
            seller_name_query: None,
            exclude_seller_name_query: None,
            shop_type_query: Some(HashSet::from_iter([ShopTypeData::AuctionHouse])),
            price_query: Some(RangeQuery {
                min: Some(500),
                max: Some(25_000),
            }),
            state_query: Some(HashSet::from_iter([ProductStateData::Sold])),
            origin_year_query: Some(RangeQuery {
                min: Some(1720.into()),
                max: Some(1790.into()),
            }),
            authenticity_query: Some(HashSet::from_iter([AuthenticityData::LaterCopy])),
            condition_query: Some(HashSet::from_iter([ConditionData::Fair])),
            provenance_query: Some(HashSet::from_iter([ProvenanceData::Claimed])),
            restoration_query: Some(HashSet::from_iter([RestorationData::Major])),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2022-01-01 0:00 UTC)),
                max: Some(datetime!(2032-01-01 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2023-01-01 0:00 UTC)),
                max: Some(datetime!(2033-01-01 0:00 UTC)),
            }),
        }),
    };

    let patch_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .json(&patch)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    let patched = patch_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(patch.name.as_ref().unwrap(), &patched.name);
    assert_eq!(
        patch.search.as_ref().unwrap().language.as_ref().unwrap(),
        &patched.search.language
    );
    assert_eq!(
        patch.search.as_ref().unwrap().currency.as_ref().unwrap(),
        &patched.search.currency
    );
    assert_eq!(
        patch
            .search
            .as_ref()
            .unwrap()
            .product_query
            .as_ref()
            .unwrap(),
        patched.search.product_query.as_ref().unwrap()
    );

    tokio::time::sleep(Duration::from_secs(30)).await;
    let patched_document =
        wait_until_document_exists(patched.user_search_filter_id.to_string()).await;
    assert_eq!(
        patched.user_search_filter_id,
        patched_document.user_search_filter_id
    );
    assert_eq!(patched.user_id, patched_document.user_id);
    assert_eq!(patched.name, patched_document.name);
    assert_eq!(patched.created, patched_document.created);
    assert_eq!(patched.updated, patched_document.updated);
    assert!(patched.updated >= posted.updated);
    assert_ne!(initial_document.query, patched_document.query);
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_delete_search_filter_and_remove_it_from_opensearch() {
    let user = create_random_test_user().await;

    let expected = PostUserSearchFilterData {
        name: "Staging sync delete".into(),
        search: ProductSearchData {
            language: LanguageData::En,
            currency: CurrencyData::Gbp,
            product_query: Some("Georgian cabinet".try_into().unwrap()),
            category_id: HashSet::from_iter([CategoryId::from("furniture")]),
            period_id: HashSet::from_iter([PeriodId::from("georgian")]),
            shop_name_query: HashSet::from_iter([ShopName::from("Delete Me Shop")]),
            exclude_shop_name_query: HashSet::from_iter([ShopName::from("Excluded Delete Shop")]),
            seller_name_query: Default::default(),
            exclude_seller_name_query: Default::default(),
            shop_type_query: HashSet::from_iter([ShopTypeData::CommercialDealer]),
            price_query: Some(RangeQuery {
                min: Some(200),
                max: Some(12000),
            }),
            state_query: HashSet::from_iter([ProductStateData::Available]),
            origin_year_query: Some(RangeQuery {
                min: Some(1714.into()),
                max: Some(1830.into()),
            }),
            authenticity_query: HashSet::from_iter([AuthenticityData::Original]),
            condition_query: HashSet::from_iter([ConditionData::Great]),
            provenance_query: HashSet::from_iter([ProvenanceData::Complete]),
            restoration_query: HashSet::from_iter([RestorationData::Minor]),
            created_query: Some(RangeQuery {
                min: Some(datetime!(2020-01-01 0:00 UTC)),
                max: Some(datetime!(2030-01-01 0:00 UTC)),
            }),
            updated_query: Some(RangeQuery {
                min: Some(datetime!(2020-01-01 0:00 UTC)),
                max: Some(datetime!(2030-01-01 0:00 UTC)),
            }),
            auction_start_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2026-01-01 0:00 UTC)),
            }),
            auction_end_query: Some(RangeQuery {
                min: Some(datetime!(2024-01-01 0:00 UTC)),
                max: Some(datetime!(2026-01-01 0:00 UTC)),
            }),
        },
    };

    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    let document = wait_until_document_exists(posted.user_search_filter_id.to_string()).await;
    assert_eq!(posted.user_search_filter_id, document.user_search_filter_id);

    let delete_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let delete_response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    wait_until_document_deleted(posted.user_search_filter_id.to_string()).await;
}
*/

// ---------------------------------------------------------------------------
// Search filter percolation
// Verifies that newly ingested products are matched against stored search
// filters and that a notification email is sent to the filter owner.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_send_email_to_user_when_product_matches_search_filter() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // Create and configure user
    let user = create_test_user("search-filter-test@example.com").await;
    tokio::time::sleep(Duration::from_secs(10)).await;
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    assert!(
        user_repository
            .get_user_record(&user.sub.into())
            .await
            .unwrap()
            .is_some()
    );
    user_repository
        .update_user_record(
            &user.sub.into(),
            UserRecordUpdate {
                first_name: Some("Thomas".into()),
                last_name: Some("Testperson".into()),
                language: Some(common::language::record::LanguageRecord::De),
                currency: Some(common::currency::record::CurrencyRecord::Eur),
                prohibited_content_consent: None,
                updated: OffsetDateTime::now_utc(),
            },
        )
        .await
        .unwrap();

    // Create a search filter that matches AVAILABLE products
    let post_url = format!(
        "{}/api/v1/me/search-filters",
        stack.api_gateway_endpoint_url
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .bearer_auth(&user.access_token)
        .json(&serde_json::json!({
            "name": "My Available Products",
            "search": {
                "language": "de",
                "currency": "EUR",
                "state": ["AVAILABLE"]
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let filter: UserSearchFilterData = post_response.json().await.unwrap();
    assert_eq!(filter.name.to_string(), "My Available Products");
    tokio::time::sleep(Duration::from_secs(30)).await;

    // Create a product with state=AVAILABLE — should match the filter and trigger email
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data.state = ProductStateData::Available;
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    upsert_products(vec![put_product_data.clone()]).await;

    assert!(wait_for_ses_email("Neues Ergebnis für", Duration::from_secs(120)).await);
}
*/

// ---------------------------------------------------------------------------
// API: Product get
// Verifies API Gateway routing, Lambda IAM access to DynamoDB/watchlist,
// slug-based routing, history endpoint, and authenticated personalization.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_anon_and_product_does_exist_for_ids() {
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let record = Faker.fake::<ProductRecord>();
    let insert_res = repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/shops/{}/products/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_product_id
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_product_id.to_string(),
        body["item"]["shopsProductId"]
    );
    assert_eq!(record.product_id.to_string(), body["item"]["productId"]);
    assert_eq!(record.event_id.to_string(), body["item"]["eventId"]);
    assert_eq!(record.url.to_string(), body["item"]["url"]);
    assert_eq!(
        record.price_gbp.unwrap(),
        body["item"]["price"]["offer"]["amount"]
    );
    assert_eq!("GBP", body["item"]["price"]["offer"]["currency"]);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_anon_and_product_does_exist_for_slug_ids() {
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let record = Faker.fake::<ProductRecord>();
    let insert_res = repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/by-slug/shops/{}/products/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_slug_id,
        record.product_slug_id
    );
    let response = reqwest::get(url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_product_id.to_string(),
        body["item"]["shopsProductId"]
    );
    assert_eq!(record.product_id.to_string(), body["item"]["productId"]);
    assert_eq!(
        record.price_gbp.unwrap(),
        body["item"]["price"]["offer"]["amount"]
    );
    assert_eq!("GBP", body["item"]["price"]["offer"]["currency"]);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_product_history() {
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let event_1_id = EventId::new();
    let event_1_price = Price::new(1000u64.into(), Currency::Eur);
    let event_1 = Event {
        aggregate_id: record.product_id,
        event_id: event_1_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceChanged(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                seller_id: record.seller_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: Some(event_1_price),
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Some(Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                }),
                old_other_price: FixedFxRate()
                    .exchange_all(Currency::Eur, 100000u64.into())
                    .unwrap(),
            },
        )),
    };
    tokio::time::sleep(Duration::from_secs(1)).await;
    let event_2_id = EventId::new();
    let event_2 = Event {
        aggregate_id: record.product_id,
        event_id: event_2_id,
        timestamp: SystemTime::now().into(),
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateChanged(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                seller_id: record.seller_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
                new_state: ProductState::Removed,
            },
        )),
    };
    let insert_res = product_repository
        .put_product_event_records([event_1.into(), event_2.into()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let response = reqwest::get(format!(
        "{}/api/v1/shops/{}/products/{}/history?currency=USD",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_product_id,
    ))
    .await
    .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    let history = body.as_array().cloned().unwrap();
    assert_eq!(2, history.len());
    assert_eq!(event_1_id.to_string(), history[0]["eventId"]);
    assert_eq!("PRICE_CHANGED", history[0]["eventType"]);
    assert_eq!("USD", history[0]["payload"]["newPrice"]["currency"]);
    assert_eq!(event_2_id.to_string(), history[1]["eventId"]);
    assert_eq!("STATE_CHANGED", history[1]["eventType"]);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_personalized_when_authenticated_and_product_exists_for_ids() {
    let user = create_random_test_user().await;

    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let record = Faker.fake::<ProductRecord>();
    let insert_res = repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/shops/{}/products/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_product_id
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_product_id.to_string(),
        body["item"]["shopsProductId"]
    );
    assert_eq!(record.product_id.to_string(), body["item"]["productId"]);
    assert!(
        !body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_personalized_when_authenticated_and_product_exists_for_slug_ids() {
    let user = create_random_test_user().await;

    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let record = Faker.fake::<ProductRecord>();
    let insert_res = repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;

    let url = format!(
        "{}/api/v1/by-slug/shops/{}/products/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_slug_id,
        record.product_slug_id
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(
        record.shops_product_id.to_string(),
        body["item"]["shopsProductId"]
    );
    assert_eq!(record.product_id.to_string(), body["item"]["productId"]);
    assert!(
        !body["userState"]["watchlist"]["watching"]
            .as_bool()
            .unwrap()
    );
    assert!(
        !body["userState"]["watchlist"]["notifications"]
            .as_bool()
            .unwrap()
    );
}

// ---------------------------------------------------------------------------
// API: Product search
// Verifies OpenSearch query routing, watchlist personalization for auth users,
// and correct currency/language serialization.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_product_search_hits_authenticated() {
    let cfn = get_cfn_output();
    let ddb_client = get_dynamodb_client().await;
    let watchlist_repository =
        WatchlistProductDynamoDbRepositoryImpl::new(ddb_client, &cfn.dynamodb_table_1_name);
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(ddb_client, &cfn.dynamodb_table_1_name);
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let product_watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &product_repository,
    );

    let now = SystemTime::now();
    let os_client = get_opensearch_client().await;
    let product_opensearch_repository = ProductOpenSearchRepositoryImpl::new(os_client);
    let expected = ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("Foo"),
        shop_slug_id: SlugId::from("Foo"),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shop_type: Faker.fake(),
        shops_product_id: ShopsProductId::new(),
        shop_name: "Hans Volkers Shop".into(),
        category_id: Faker.fake(),
        category_name_de: Faker.fake(),
        category_name_en: Faker.fake(),
        category_name_fr: Faker.fake(),
        category_name_es: Faker.fake(),
        category_name_it: Faker.fake(),
        period_id: Faker.fake(),
        period_name_de: Faker.fake(),
        period_name_en: Faker.fake(),
        period_name_fr: Faker.fake(),
        period_name_es: Faker.fake(),
        period_name_it: Faker.fake(),
        title_native: TextDocument {
            text: "Chopin Etudes Op.10 1833".to_string(),
            language: LanguageDocument::De,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        description_it: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
        auction_start: None,
        auction_end: None,
        created: now.into(),
        updated: now.into(),
    };
    let mut all = fake::vec![ProductDocument; 10];
    all.push(expected.clone());
    let insert_res = product_opensearch_repository
        .create_product_documents(all)
        .await
        .unwrap();
    assert!(!insert_res.errors);
    os_client
        .indices()
        .refresh(IndicesRefreshParts::Index(&["products"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let product_slug_id = SlugId::from("Chopin Etudes Op.10 1833");
    let shop_slug_id = SlugId::from(&expected.shop_name);
    let ddb_materialized = ProductRecord {
        pk: product_record::mk_pk(&expected.shop_id, &expected.shops_product_id),
        sk: product_record::mk_sk().to_owned(),
        gsi2_pk: mk_gsi2_pk(&shop_slug_id, &product_slug_id),
        gsi2_sk: mk_gsi2_sk().to_owned(),
        product_id: expected.product_id,
        product_slug_id,
        shop_slug_id,
        event_id: expected.event_id,
        shop_id: expected.shop_id,
        shops_product_id: expected.shops_product_id.clone(),
        shop_name: expected.shop_name.clone(),
        shop_type: Faker.fake(),
        category_id: Faker.fake(),
        category_name_de: Faker.fake(),
        category_name_en: Faker.fake(),
        category_name_fr: Faker.fake(),
        category_name_es: Faker.fake(),
        category_name_it: Faker.fake(),
        period_id: Faker.fake(),
        period_name_de: Faker.fake(),
        period_name_en: Faker.fake(),
        period_name_fr: Faker.fake(),
        period_name_es: Faker.fake(),
        period_name_it: Faker.fake(),
        title_native: TextRecord {
            text: "Chopin Etudes Op.10 1833".to_owned(),
            language: LanguageRecord::De,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_owned()),
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_native: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        description_it: None,
        price_native: Some(PriceRecord {
            currency: CurrencyRecord::Eur,
            amount: 1400000,
        }),
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_native: Faker.fake(),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_native: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateRecord::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        embedding: Some(fake::vec![f32; 768]),
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
        auction_start: None,
        auction_end: None,
        created: now.into(),
        updated: now.into(),
    };
    let ddb_res = product_repository
        .put_product_records([ddb_materialized].into())
        .await
        .unwrap();
    assert!(ddb_res.unprocessed_items.unwrap_or_default().is_empty());

    let user = create_random_test_user().await;
    product_watchlist_service
        .create_watchlist_product(
            &user.sub.into(),
            &expected.shop_id,
            &expected.shops_product_id,
        )
        .await
        .unwrap();

    let search_filter = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Chopin Etudes Op.10".try_into().unwrap()),
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: ["Hans Volkers Shop".into()].into(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ProductStateData::Available, ProductStateData::Listed].into(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        auction_start_query: None,
        auction_end_query: None,
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999-01-02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/products/search?sort=created&order=asc&size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search_filter)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(1, body["size"]);
    assert_eq!(1, body["total"]);
    let item = body["items"].as_array().unwrap()[0]["item"].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(
        expected.shops_product_id.to_string(),
        item["shopsProductId"]
    );
    assert_eq!(expected.product_id.to_string(), item["productId"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
    let user_state = body["items"].as_array().unwrap()[0]["userState"].clone();
    assert!(user_state["watchlist"]["watching"].as_bool().unwrap());
    assert!(!user_state["watchlist"]["notifications"].as_bool().unwrap());
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_product_search_hits_anon() {
    let os_client = get_opensearch_client().await;
    let repository = ProductOpenSearchRepositoryImpl::new(os_client);
    let now = SystemTime::now();
    let expected = ProductDocument {
        product_id: ProductId::new(),
        product_slug_id: SlugId::from("Foo"),
        shop_slug_id: SlugId::from("Foo"),
        event_id: EventId::new(),
        shop_id: ShopId::new(),
        shops_product_id: ShopsProductId::new(),
        shop_name: "Hans Volkers Shop".into(),
        shop_type: Faker.fake(),
        category_id: Faker.fake(),
        category_name_de: Faker.fake(),
        category_name_en: Faker.fake(),
        category_name_fr: Faker.fake(),
        category_name_es: Faker.fake(),
        category_name_it: Faker.fake(),
        period_id: Faker.fake(),
        period_name_de: Faker.fake(),
        period_name_en: Faker.fake(),
        period_name_fr: Faker.fake(),
        period_name_es: Faker.fake(),
        period_name_it: Faker.fake(),
        title_native: TextDocument {
            text: "Chopin Etudes Op.10 1833".to_string(),
            language: LanguageDocument::De,
        },
        title_de: Some("Chopin Etudes Op.10 1833".to_string()),
        title_en: None,
        title_fr: None,
        title_es: None,
        title_it: None,
        description_de: None,
        description_en: None,
        description_fr: None,
        description_es: None,
        description_it: None,
        price_eur: Some(1400000),
        price_usd: Some(1500000),
        price_gbp: Some(1600000),
        price_aud: Some(1700000),
        price_cad: Some(1800000),
        price_nzd: Some(1990000),
        price_estimate_min_eur: Faker.fake(),
        price_estimate_min_usd: Faker.fake(),
        price_estimate_min_gbp: Faker.fake(),
        price_estimate_min_aud: Faker.fake(),
        price_estimate_min_cad: Faker.fake(),
        price_estimate_min_nzd: Faker.fake(),
        price_estimate_max_eur: Faker.fake(),
        price_estimate_max_usd: Faker.fake(),
        price_estimate_max_gbp: Faker.fake(),
        price_estimate_max_aud: Faker.fake(),
        price_estimate_max_cad: Faker.fake(),
        price_estimate_max_nzd: Faker.fake(),
        state: ProductStateDocument::Available,
        url: Url::parse("https://hans-volker.com/chopin-etudes-op10-1833").unwrap(),
        images: vec![],
        embedding: None,
        origin_year_min: None,
        origin_year: None,
        origin_year_max: None,
        authenticity: Default::default(),
        condition: Default::default(),
        provenance: Default::default(),
        restoration: Default::default(),
        auction_start: None,
        auction_end: None,
        created: now.into(),
        updated: now.into(),
    };
    let mut all = fake::vec![ProductDocument; 10];
    all.push(expected.clone());
    let insert_res = repository.create_product_documents(all).await.unwrap();
    assert!(!insert_res.errors);
    os_client
        .indices()
        .refresh(IndicesRefreshParts::Index(&["products"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search_filter = ProductSearchData {
        language: LanguageData::De,
        currency: CurrencyData::Eur,
        product_query: Some("Chopin Etudes Op.10".try_into().unwrap()),
        category_id: Default::default(),
        period_id: Default::default(),
        shop_name_query: ["Hans Volkers Shop".into()].into(),
        exclude_shop_name_query: Default::default(),
        seller_name_query: Default::default(),
        exclude_seller_name_query: Default::default(),
        shop_type_query: Default::default(),
        price_query: Some(RangeQuery {
            min: None,
            max: Some(99999999),
        }),
        state_query: [ProductStateData::Available, ProductStateData::Listed].into(),
        origin_year_query: None,
        authenticity_query: Default::default(),
        condition_query: Default::default(),
        provenance_query: Default::default(),
        restoration_query: Default::default(),
        auction_start_query: None,
        auction_end_query: None,
        created_query: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999-01-02 0:00 UTC)),
        }),
        updated_query: None,
    };

    let url = format!(
        "{}/api/v1/products/search?sort=created&order=asc&size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search_filter)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(1, body["size"]);
    assert_eq!(1, body["total"]);
    let item = body["items"].as_array().unwrap()[0]["item"].clone();
    assert_eq!(expected.shop_id.to_string(), item["shopId"]);
    assert_eq!(expected.product_id.to_string(), item["productId"]);
    assert_eq!(expected.price_eur.unwrap(), item["price"]["amount"]);
    assert_eq!("EUR", item["price"]["currency"]);
    assert!(body["items"].as_array().unwrap()[0]["userState"].is_null());
}
*/

// ---------------------------------------------------------------------------
// API: Product similar
// Verifies the ANN/KNN endpoint, 202 when embeddings are missing, and
// watchlist personalization for authenticated users.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_202_when_similar_products_embedding_not_computed() {
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut product_record: ProductRecord = Faker.fake();
    product_record.embedding = None;
    let ddb_insert_res = product_dynamodb_repository
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();
    assert!(
        ddb_insert_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let product_document: ProductDocument = product_record.clone().into();
    let mut product_documents = fake::vec![ProductDocument; 10];
    for doc in &mut product_documents {
        doc.embedding = None;
    }
    product_documents.push(product_document);
    let os_insert_res = product_opensearch_repository
        .create_product_documents(product_documents)
        .await
        .unwrap();
    assert!(!os_insert_res.errors);

    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&["products"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(20)).await;

    let url = format!(
        "{}/api/v1/shops/{}/products/{}/similar",
        get_cfn_output().api_gateway_endpoint_url,
        product_record.shop_id,
        product_record.shops_product_id,
    );
    let response = reqwest::Client::new().get(url).send().await.unwrap();
    assert_eq!(202, response.status().as_u16());
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_similar_products_computed_for_anon() {
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut product_record: ProductRecord = Faker.fake();
    product_record.embedding = Some(EXAMPLE_EMBEDDING.into());
    let ddb_insert_res = product_dynamodb_repository
        .put_product_records([product_record.clone()].into())
        .await
        .unwrap();
    assert!(
        ddb_insert_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    let product_document: ProductDocument = product_record.clone().into();
    let mut product_documents = fake::vec![ProductDocument; 10];
    for doc in &mut product_documents {
        doc.title_native = TextDocument {
            text: "My expected english title".into(),
            language: LanguageDocument::En,
        };
        doc.embedding = Some(EXAMPLE_EMBEDDING.into());
        doc.title_en = Some("My expected english title".into());
    }
    product_documents.push(product_document);
    let os_insert_res = product_opensearch_repository
        .create_product_documents(product_documents.clone())
        .await
        .unwrap();
    assert!(!os_insert_res.errors);

    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&["products"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(20)).await;

    let url = format!(
        "{}/api/v1/shops/{}/products/{}/similar?currency=USD",
        get_cfn_output().api_gateway_endpoint_url,
        product_record.shop_id,
        product_record.shops_product_id,
    );
    let response = reqwest::Client::new()
        .get(url)
        .query(&[("language", "en-US")])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status().as_u16());

    let actual: Vec<PersonalizedData<GetProductSummaryData, ProductUserStateData>> =
        response.json().await.unwrap();
    // ANN results are approximate, but all returned products must come from our seeded set
    assert!(actual.iter().all(|a| {
        product_documents
            .iter()
            .any(|e| e.product_id == a.item.product_id)
    }));
    assert!(
        actual
            .iter()
            .all(|a| &a.item.title.text == "My expected english title")
    );
    assert!(
        actual
            .iter()
            .filter_map(|a| a.item.price)
            .all(|p| p.currency == CurrencyData::Usd)
    );
    assert!(actual.iter().all(|a| a.user_state.is_none()));
}

#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_and_personalize_similar_products_for_authenticated() {
    let user = create_random_test_user().await;
    let user_id: UserId = user.sub.into();
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let get_product_service = GetProductServiceImpl::new(&product_dynamodb_repository);
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &product_dynamodb_repository,
    );

    let mut product_record: ProductRecord = Faker.fake();
    product_record.embedding = Some(EXAMPLE_EMBEDDING.into());
    let product_records = fake::vec![ProductRecord; 5];
    let ddb_insert_res = product_dynamodb_repository
        .put_product_records(
            [vec![product_record.clone()], product_records.clone()]
                .concat()
                .try_into()
                .unwrap(),
        )
        .await
        .unwrap();
    assert!(
        ddb_insert_res
            .unprocessed_items
            .unwrap_or_default()
            .is_empty()
    );

    for pr in product_records.iter() {
        watchlist_service
            .create_watchlist_product(
                &user_id,
                &pr.shop_id,
                &pr.shops_product_id,
            )
            .await
            .unwrap();
    }

    let product_document: ProductDocument = product_record.clone().into();
    let mut product_documents = product_records
        .clone()
        .into_iter()
        .map(ProductDocument::from)
        .collect::<Vec<_>>();
    for doc in &mut product_documents {
        doc.title_native = TextDocument {
            text: "My expected german title".into(),
            language: LanguageDocument::De,
        };
        doc.embedding = Some(EXAMPLE_EMBEDDING.into());
        doc.title_de = Some("My expected german title".into());
    }
    product_documents.push(product_document);
    let os_insert_res = product_opensearch_repository
        .create_product_documents(product_documents.clone())
        .await
        .unwrap();
    assert!(!os_insert_res.errors);
    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&["products"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(20)).await;

    let url = format!(
        "{}/api/v1/shops/{}/products/{}/similar?currency=EUR",
        get_cfn_output().api_gateway_endpoint_url,
        product_record.shop_id,
        product_record.shops_product_id,
    );
    let response = reqwest::Client::new()
        .get(url)
        .query(&[("language", "de")])
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status().as_u16());

    let actual: Vec<PersonalizedData<GetProductSummaryData, ProductUserStateData>> =
        response.json().await.unwrap();
    assert!(actual.iter().all(|a| {
        product_documents
            .iter()
            .any(|e| e.product_id == a.item.product_id)
    }));
    assert!(
        actual
            .iter()
            .all(|a| &a.item.title.text == "My expected german title")
    );
    assert!(
        actual
            .iter()
            .filter_map(|a| a.item.price)
            .all(|p| p.currency == CurrencyData::Eur)
    );
    assert!(actual.iter().all(|a| a.user_state.is_some()));
    assert!(
        actual
            .iter()
            .all(|a| a.user_state.unwrap().watchlist.watching)
    );
}
*/

// ---------------------------------------------------------------------------
// API: Product watchlist
// Verifies Cognito-protected endpoints, full CRUD lifecycle, and DynamoDB
// access for watchlist records.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_post_get_patch_delete_watchlist_product() {
    let user = create_random_test_user().await;

    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let materialized = Faker.fake::<ProductRecord>();
    let put_res = repository
        .put_product_records([materialized.clone()].into())
        .await
        .unwrap();
    assert!(put_res.unprocessed_items.unwrap_or_default().is_empty());

    // POST
    let post_url = format!(
        "{}/api/v1/me/watchlist",
        get_cfn_output().api_gateway_endpoint_url
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ProductKeyData {
            shop_id: materialized.shop_id,
            shops_product_id: materialized.shops_product_id.clone(),
        })
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());

    // GET
    let get_url = format!(
        "{}/api/v1/me/watchlist?currency=EUR&sort=created&order=desc",
        get_cfn_output().api_gateway_endpoint_url
    );
    let get_response = reqwest::Client::new()
        .get(get_url.clone())
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>>>()
        .await
        .unwrap();
    assert_eq!(1, gotten.items.len());
    assert_eq!(&materialized.product_id, &gotten.items[0].item.product_id);
    assert_eq!(&materialized.shop_id, &gotten.items[0].item.shop_id);
    assert_eq!(
        &materialized.shops_product_id,
        &gotten.items[0].item.shops_product_id
    );
    assert_eq!(1, gotten.total.unwrap());

    // PATCH
    let patch_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        materialized.shop_id,
        materialized.shops_product_id
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url.clone())
        .bearer_auth(&user.access_token)
        .json(&WatchlistProductPatch {
            notifications: Some(!gotten.items[0].user_state.unwrap().watchlist.notifications),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patch_res = patch_response
        .json::<PersonalizedData<GetProductData, ProductUserStateData>>()
        .await
        .unwrap();
    assert_eq!(materialized.shop_id, patch_res.item.shop_id);
    assert_eq!(
        materialized.shops_product_id,
        patch_res.item.shops_product_id
    );
    assert_eq!(materialized.product_id, patch_res.item.product_id);

    // DELETE
    let delete_url = format!(
        "{}/api/v1/me/watchlist/{}/{}",
        get_cfn_output().api_gateway_endpoint_url,
        materialized.shop_id,
        materialized.shops_product_id
    );
    let delete_response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    // GET after delete
    let get_response = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>>>()
        .await
        .unwrap();
    assert!(gotten.items.is_empty());
    assert_eq!(0, gotten.total.unwrap_or(0));
}

// ---------------------------------------------------------------------------
// API: Search filter
// Verifies Cognito-protected endpoints and full CRUD lifecycle for user
// search filters stored in DynamoDB.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_get_all_search_filters_when_authorized() {
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

    let user = create_random_test_user().await;
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user.sub.into(), update_cmd)
        .await
        .unwrap();

    let expected1 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected1_name = Faker.fake::<UserSearchFilterName>();
    let expected2 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected2_name = Faker.fake::<UserSearchFilterName>();
    service
        .create_user_search_filter(&user.sub.into(), expected1_name.clone(), expected1.clone())
        .await
        .unwrap();
    service
        .create_user_search_filter(&user.sub.into(), expected2_name.clone(), expected2.clone())
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/me/search-filters?sort=created&order=asc",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<PaginatedData<UserSearchFilterData>>()
        .await
        .unwrap();
    assert_eq!(2, actual.total.unwrap());
    let actual1 = actual.items.first().unwrap().clone();
    let actual2 = actual.items.get(1).unwrap().clone();
    assert_eq!(
        expected1,
        product::core::product_search::ProductSearch::from(actual1.search)
    );
    assert_eq!(
        expected2,
        product::core::product_search::ProductSearch::from(actual2.search)
    );
    assert_eq!(expected1_name, actual1.name);
    assert_eq!(expected2_name, actual2.name);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_post_get_patch_delete_search_filter() {
    let user = create_random_test_user().await;
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    let user_repository = UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    user_service
        .update_user(&user.sub.into(), update_cmd)
        .await
        .unwrap();

    // POST
    let expected = Faker.fake::<PostUserSearchFilterData>();
    let post_url = format!(
        "{}/api/v1/me/search-filters",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&expected)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(&expected.name, &posted.name);
    assert_eq!(&expected.search, &posted.search);
    assert_eq!(user.sub.to_string(), posted.user_id.to_string());

    // GET one
    let get_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let get_response = reqwest::Client::new()
        .get(get_url.clone())
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(&expected.search, &gotten.search);
    assert_eq!(posted.user_search_filter_id, gotten.user_search_filter_id);

    // PATCH
    let patch = PatchUserSearchFilterData {
        name: None,
        notifications: None,
        search: Some(PatchProductSearchData {
            language: Some(LanguageData::Fr),
            currency: None,
            product_query: Some("weesl bee wuff".try_into().unwrap()),
            category_id: None,
            period_id: None,
            shop_name_query: None,
            exclude_shop_name_query: None,
            seller_name_query: None,
            exclude_seller_name_query: None,
            shop_type_query: None,
            price_query: None,
            state_query: None,
            origin_year_query: None,
            authenticity_query: None,
            condition_query: None,
            provenance_query: None,
            restoration_query: None,
            auction_start_query: None,
            auction_end_query: None,
            created_query: None,
            updated_query: None,
        }),
    };
    let patch_url = format!(
        "{}/api/v1/me/search-filters/{}",
        get_cfn_output().api_gateway_endpoint_url,
        posted.user_search_filter_id
    );
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .bearer_auth(&user.access_token)
        .json(&patch)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patched = patch_response.json::<UserSearchFilterData>().await.unwrap();
    assert_eq!(
        &patch.search.clone().unwrap().language.unwrap(),
        &patched.search.language
    );
    assert_eq!(
        &patch.search.unwrap().product_query.unwrap(),
        &patched.search.product_query.unwrap()
    );
    assert_eq!(posted.user_search_filter_id, patched.user_search_filter_id);

    // DELETE
    let delete_response = reqwest::Client::new()
        .delete(get_url.clone())
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    // GET after delete → 404
    let get_after_delete = reqwest::Client::new()
        .get(get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(404, get_after_delete.status());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_get_search_filter_products_when_authorized() {
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_repository = UserDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let service = UserSearchFilterServiceImpl::new(&repository, &user_service);

    let user = create_random_test_user().await;
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user.sub.into(), update_cmd)
        .await
        .unwrap();
    let search_filter = service
        .create_user_search_filter(
            &user.sub.into(),
            Faker.fake(),
            Faker.fake::<product::core::product_search::ProductSearch>(),
        )
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/me/search-filters/{}/products?language=de&currency=EUR&sort=created&order=asc&size=10",
        get_cfn_output().api_gateway_endpoint_url,
        search_filter.user_search_filter_id,
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<TimeCursoredData<PersonalizedData<GetProductData, ProductUserStateData>>>()
        .await
        .unwrap();
    assert_eq!(0, actual.total.unwrap());
    assert!(actual.items.is_empty());
}

// ---------------------------------------------------------------------------
// API: Shop
// Verifies API Gateway routing and Lambda IAM access for shop GET and
// OpenSearch-backed shop search.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_shop_search_hits() {
    let os_client = get_opensearch_client().await;
    let repository = ShopOpenSearchRepositoryImpl::new(os_client);
    let expected = Faker.fake::<ShopDocument>();
    let mut all = fake::vec![ShopDocument; 10];
    all.push(expected.clone());

    for shop in all {
        repository.index_shop_document(shop).await.unwrap();
    }
    os_client
        .indices()
        .refresh(IndicesRefreshParts::Index(&["shops"]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
    tokio::time::sleep(Duration::from_secs(3)).await;

    let search = ShopSearchData {
        shop_name_query: Some(expected.name.to_string().try_into().unwrap()),
        shop_type_query: Default::default(),
        created: Some(RangeQuery {
            min: None,
            max: Some(datetime!(2999-01-02 0:00 UTC)),
        }),
        updated: None,
    };

    let url = format!(
        "{}/api/v1/shops/search?size=5",
        get_cfn_output().api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .json(&search)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
*/

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_shop_get_by_id() {
    let shop = prepare_test_shop().await;

    let url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        shop.shop_id,
    );
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetShopData>().await.unwrap();
    assert_eq!(shop.shop_id, body.shop_id);
    assert_eq!(shop.shop_slug_id, body.shop_slug_id);
    assert_eq!(shop.name, body.name);
    assert_eq!(shop.domains, body.domains);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_shop_get_by_slug() {
    let shop = prepare_test_shop().await;

    let url = format!(
        "{}/api/v1/by-slug/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        shop.shop_slug_id,
    );
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetShopData>().await.unwrap();
    assert_eq!(shop.shop_id, body.shop_id);
    assert_eq!(shop.shop_slug_id, body.shop_slug_id);
    assert_eq!(shop.name, body.name);
    assert_eq!(shop.domains, body.domains);
}

// ---------------------------------------------------------------------------
// API: Product Classification
// Verifies API Gateway routing and Lambda IAM access for category and period
// GET-by-id and GET-all endpoints (DynamoDB-backed).
// ---------------------------------------------------------------------------

/**
#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_category_get_by_id() {
    let stack = get_cfn_output();
    let repository = CategoryDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let record: CategoryRecord = Faker.fake();
    repository
        .put_category_record(record.clone())
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/categories/{}",
        stack.api_gateway_endpoint_url, record.category_id,
    );
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetCategoryData>().await.unwrap();
    assert_eq!(record.category_id, body.category_id);
    assert_eq!(record.category_key, body.category_key);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_category_get_all() {
    let stack = get_cfn_output();
    let repository = CategoryDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let record1: CategoryRecord = Faker.fake();
    let record2: CategoryRecord = Faker.fake();
    repository
        .put_category_record(record1.clone())
        .await
        .unwrap();
    repository
        .put_category_record(record2.clone())
        .await
        .unwrap();

    let url = format!("{}/api/v1/categories", stack.api_gateway_endpoint_url,);
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response
        .json::<Vec<GetCategorySummaryData>>()
        .await
        .unwrap();
    assert!(body.len() >= 2);
    assert!(body.iter().any(|c| c.category_id == record1.category_id));
    assert!(body.iter().any(|c| c.category_id == record2.category_id));
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_period_get_by_id() {
    let stack = get_cfn_output();
    let repository = PeriodDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let record: PeriodRecord = Faker.fake();
    repository.put_period_record(record.clone()).await.unwrap();

    let url = format!(
        "{}/api/v1/periods/{}",
        stack.api_gateway_endpoint_url, record.period_id,
    );
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetPeriodData>().await.unwrap();
    assert_eq!(record.period_id, body.period_id);
    assert_eq!(record.period_key, body.period_key);
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_period_get_all() {
    let stack = get_cfn_output();
    let repository = PeriodDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let record1: PeriodRecord = Faker.fake();
    let record2: PeriodRecord = Faker.fake();
    repository.put_period_record(record1.clone()).await.unwrap();
    repository.put_period_record(record2.clone()).await.unwrap();

    let url = format!("{}/api/v1/periods", stack.api_gateway_endpoint_url,);
    let response = reqwest::get(&url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<Vec<GetPeriodSummaryData>>().await.unwrap();
    assert!(body.len() >= 2);
    assert!(body.iter().any(|p| p.period_id == record1.period_id));
    assert!(body.iter().any(|p| p.period_id == record2.period_id));
}
*/

// ---------------------------------------------------------------------------
// API: Notification
// Verifies Cognito-protected notification endpoints: get, patch-one,
// patch-all, delete-one, delete-all, seeding data directly via DynamoDB.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_get_patch_delete_notifications() {
    let user = create_random_test_user().await;
    let repository = NotificationDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );

    // Seed notifications directly via DynamoDB
    let mut record1 = Faker.fake::<NotificationRecord>();
    record1.pk = notification::dynamodb::notification_record::mk_pk(&user.sub.into());
    record1.user_id = user.sub.into();
    record1.seen = false;
    repository
        .put_notification_record(record1.clone())
        .await
        .unwrap();

    let mut record2 = Faker.fake::<NotificationRecord>();
    record2.pk = notification::dynamodb::notification_record::mk_pk(&user.sub.into());
    record2.user_id = user.sub.into();
    record2.seen = false;
    repository
        .put_notification_record(record2.clone())
        .await
        .unwrap();

    // GET all
    let get_url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert_eq!(2, gotten.items.len());
    assert_eq!(Some(2), gotten.total);
    assert!(gotten.items.iter().all(|n| !n.seen));

    // PATCH one (mark as seen)
    let patch_one_url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        record1.origin_event_id,
    );
    let patch_one_response = reqwest::Client::new()
        .patch(patch_one_url)
        .bearer_auth(&user.access_token)
        .json(&PatchNotificationData { seen: Some(true) })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_one_response.status());
    let patched_one = patch_one_response
        .json::<GetNotificationData>()
        .await
        .unwrap();
    assert_eq!(record1.origin_event_id, patched_one.origin_event_id);
    assert!(patched_one.seen);

    // PATCH all (mark all as seen)
    let patch_all_url = format!(
        "{}/api/v1/me/notifications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let patch_all_response = reqwest::Client::new()
        .patch(&patch_all_url)
        .bearer_auth(&user.access_token)
        .json(&PatchNotificationData { seen: Some(true) })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_all_response.status());
    let patched_all = patch_all_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert!(patched_all.items.iter().all(|n| n.seen));

    // DELETE one
    let delete_one_url = format!(
        "{}/api/v1/me/notifications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        record1.origin_event_id,
    );
    let delete_one_response = reqwest::Client::new()
        .delete(delete_one_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_one_response.status());

    // GET after delete-one: 1 remains
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let after_delete_one = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert_eq!(1, after_delete_one.items.len());

    // DELETE all
    let delete_all_response = reqwest::Client::new()
        .delete(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_all_response.status());

    // GET after delete-all: none remain
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let after_delete_all = get_response
        .json::<EventIdCursoredData<GetNotificationData>>()
        .await
        .unwrap();
    assert!(after_delete_all.items.is_empty());
    assert_eq!(0, after_delete_all.total.unwrap_or(0));
}

// ---------------------------------------------------------------------------
// API: Partner Product Creation
// Verifies API Gateway routing and Lambda execution for the partner product
// creation endpoint with x-api-key authentication (no Cognito JWT).
// ---------------------------------------------------------------------------

async fn prepare_partner_shop() -> (ShopRecord, PartnerShopApiKey) {
    let stack = get_cfn_output();
    let api_key = PartnerShopApiKey::new();
    let hashed: HashedPartnerShopApiKey = api_key.clone().into();
    let mut record: ShopRecord = Faker.fake();
    record.partner_api_key_short = Some(hashed.short_token().to_string());
    record.partner_api_key_long_hash = Some(hashed.long_token_hash().to_string());
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    dynamodb_repository
        .put_shop_record(record.clone())
        .await
        .unwrap();
    (record, api_key)
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_post_products() {
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();

    let url = format!(
        "{}/api/v1/shops/{}/products",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );
    let response = reqwest::Client::new()
        .post(&url)
        .header("x-api-key", &api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-product-1",
            "title": { "text": "Test Product", "language": "en" },
            "description": { "text": "A test product", "language": "en" },
            "state": "AVAILABLE",
            "url": "https://example.com/product/1",
            "images": ["https://example.com/img.jpg"]
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}

// ─── Product Pipeline Embed Text (Lambda) ─────────────────────────────────────

#[localstack_test(services = [Cloudformation()])]
async fn should_embed_product_when_domain_created_event_triggers_pipeline() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    // 1. Create product via command service (triggers DOMAIN_CREATED event)
    let mut create_cmd: CreateProductCommand = Faker.fake();
    create_cmd.shop_id = shop.shop_id;
    create_cmd.shop_name = shop.name.clone();
    create_cmd.shop_type = shop.shop_type;

    create_products(vec![create_cmd.clone()]).await;

    // 2. Wait for product to be materialized first
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap();

        if materialized.is_some() {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not materialized after 60s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // 3. Wait for the embed-text Lambda to produce an enrichment event
    //    which the materialize-dynamodb Lambda then materializes into the product record.
    //    The MockMultimodalEmbeddingService returns vec![0.42f32; 768].
    let expected_embedding = vec![0.42f32; 768];
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap();

        if let Some(record) = materialized
            && record.embedding.is_some()
        {
            assert_eq!(
                expected_embedding,
                record.embedding.unwrap(),
                "Embedding should match MockMultimodalEmbeddingService output"
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with embedding after 120s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

/* Classify acceptance test disabled — LocalStack cannot route Lambda→OpenSearch traffic.
#[localstack_test(services = [Cloudformation()])]
async fn should_classify_product_when_embedded_text_event_triggers_pipeline() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;
    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    // 1. Create product via command service (triggers DOMAIN_CREATED event)
    let mut create_cmd: CreateProductCommand = Faker.fake();
    create_cmd.shop_id = shop.shop_id;
    create_cmd.shop_name = shop.name.clone();
    create_cmd.shop_type = shop.shop_type;

    create_products(vec![create_cmd.clone()]).await;

    // 2. Wait for embedding to be materialized (comes from embed-text Lambda)
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap();

        if let Some(record) = materialized
            && record.embedding.is_some()
        {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with embedding after 120s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }

    // 3. Wait for the classify Lambda to produce enrichment events
    //    (ClassifiedCategory + ClassifiedPeriod) which the materialize-dynamodb Lambda
    //    then materializes into the product record.
    //    The MockClassificationService returns the first candidate for both.
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &create_cmd.shops_product_id)
            .await
            .unwrap();

        if let Some(record) = materialized
            && record.category_id.is_some()
            && record.period_id.is_some()
        {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with category_id and period_id after 120s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}
*/

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_patch_products() {
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        get_cfn_output().dynamodb_table_1_name.as_str(),
    );
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();

    let url = format!(
        "{}/api/v1/shops/{}/products",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );

    // First create the product so we can update it
    let mut product_record = Faker.fake::<ProductRecord>();
    product_record.shop_id = shop_record.shop_id;
    product_record.shops_product_id = "acceptance-test-patch-product-1".into();
    product_record.pk =
        product_record::mk_pk(&shop_record.shop_id, &product_record.shops_product_id);
    product_record.sk = product_record::mk_sk().to_owned();
    product_record.state = product::dynamodb::product_state_record::ProductStateRecord::Available;
    product_repository
        .put_product_records([product_record].into())
        .await
        .unwrap();

    // Then update the product via PATCH
    let response = reqwest::Client::new()
        .patch(&url)
        .header("x-api-key", &api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-patch-product-1",
            "state": "SOLD"
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_put_products_when_creating_new() {
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();

    let url = format!(
        "{}/api/v1/shops/{}/products",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );
    let response = reqwest::Client::new()
        .put(&url)
        .header("x-api-key", &api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-put-product-1",
            "title": { "text": "Test Product via PUT", "language": "en" },
            "description": { "text": "A test product via upsert", "language": "en" },
            "state": "AVAILABLE",
            "url": "https://example.com/product/1",
            "images": ["https://example.com/img.jpg"]
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_put_products_when_updating_existing() {
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        get_cfn_output().dynamodb_table_1_name.as_str(),
    );
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();

    let url = format!(
        "{}/api/v1/shops/{}/products",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );

    // First create the product so we can update it via PUT
    let mut product_record = Faker.fake::<ProductRecord>();
    product_record.shop_id = shop_record.shop_id;
    product_record.shops_product_id = "acceptance-test-put-existing-product-1".into();
    product_record.pk =
        product_record::mk_pk(&shop_record.shop_id, &product_record.shops_product_id);
    product_record.sk = product_record::mk_sk().to_owned();
    product_record.state = product::dynamodb::product_state_record::ProductStateRecord::Available;
    product_repository
        .put_product_records([product_record].into())
        .await
        .unwrap();

    // Then update the product via PUT
    let response = reqwest::Client::new()
        .put(&url)
        .header("x-api-key", &api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-put-existing-product-1",
            "state": "SOLD"
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body: serde_json::Value = response.json().await.unwrap();
    assert!(body["errors"].as_object().unwrap().is_empty());
}
