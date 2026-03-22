use aws_tests_common::get_cfn_output;
use common::{
    api::collection::PutCollectionData,
    batch::Batch,
    category_key::CategoryId,
    currency::{data::CurrencyData, domain::Currency, record::CurrencyRecord},
    domain::Domain,
    event::Event,
    event_id::EventId,
    language::{
        data::{LanguageData, LocalizedTextData},
        document::{LanguageDocument, TextDocument},
        record::{LanguageRecord, TextRecord},
    },
    pagination::{cursor::api::TimeCursoredData, page::api::PaginatedData},
    period_key::PeriodId,
    personalized::api::PersonalizedData,
    price::{
        domain::{FixedFxRate, FxRate, Price},
        record::PriceRecord,
    },
    product_id::{ProductId, api::ProductKeyData},
    product_state::domain::ProductState,
    query::range_query::RangeQuery,
    shop_id::ShopId,
    shop_name::ShopName,
    shops_product_id::ShopsProductId,
    slug_id::SlugId,
    sort::{Sort, SortOrder},
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
use opensearch::{GetParts, indices::IndicesRefreshParts};
use product::{
    core::{
        product_event::{
            ProductEvent, ProductEventPayload,
            domain::{
                ProductDomainEventPayload, ProductPriceChangeDomainEventPayload,
                ProductStateChangeDomainEventPayload,
            },
            enrichment::{
                EmbeddedTextProductEnrichmentEventPayload, ProductEnrichmentEventPayload,
            },
            policy::{ProductPolicyEventPayload, ProhibitedContentProductPolicyEventPayload},
        },
        product_search::ProductSearch,
        prohibited_content::{ProhibitedContent, ProhibitedContentReason},
        sort_product_field::SortProductField,
    },
    data::{
        authenticity_data::AuthenticityData, condition_data::ConditionData,
        get_summary_data::GetProductSummaryData, product_search_data::ProductSearchData,
        product_state_data::ProductStateData, provenance_data::ProvenanceData,
        put_data::PutProductData, restoration_data::RestorationData,
        user_state_data::ProductUserStateData,
    },
    dynamodb::{
        product_event_record::ProductEventRecord,
        product_image_record::ProductImageRecord,
        product_record::{self as product_record, ProductRecord, mk_gsi2_pk, mk_gsi2_sk, mk_pk},
        product_state_record::ProductStateRecord,
        prohibited_content_record::ProhibitedContentRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
    opensearch::{
        product_document::ProductDocument,
        product_state_document::ProductStateDocument,
        prohibited_content_document::ProhibitedContentDocument,
        repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
    },
    service::get_service::GetProductServiceImpl,
};
use product_watchlist::{
    data::watchlist_product_data::WatchlistProductData,
    dynamodb::repository::{
        WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
    },
    service::product_watchlist_service::{ProductWatchListService, ProductWatchListServiceImpl},
};
use product_watchlist_api::{
    watchlist_get::WatchlistProductDataView, watchlist_patch::WatchlistProductPatch,
};
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
use shop::{
    core::shop::Shop,
    data::{
        get_shop_data::GetShopData, patch_shop_data::PatchShopData, post_shop_data::PostShopData,
        shop_search_data::ShopSearchData, shop_type_data::ShopTypeData,
    },
    dynamodb::{
        repository::{ShopDynamoDbRepository, ShopDynamoDbRepositoryImpl},
        shop_record::ShopRecord,
    },
    opensearch::{
        repository::{ShopOpenSearchRepository, ShopOpenSearchRepositoryImpl},
        shop_document::ShopDocument,
    },
};
use std::time::{Duration, Instant, SystemTime};
use test_api::*;
use time::{OffsetDateTime, macros::datetime};
use url::Url;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::{
        repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
        user_record_update::UserRecordUpdate,
    },
};

// Shared 1024-dimensional text embedding used across multiple tests.
// Values are real embedding coordinates that produce meaningful ANN results in OpenSearch.
const EXAMPLE_EMBEDDING: [f32; 1024] = [
    0.0003272566,
    0.057399165,
    -0.03456967,
    -0.0106262015,
    -0.014141742,
    0.010457292,
    0.04659525,
    0.012118102,
    -0.01775892,
    -0.0030063824,
    -0.0026424518,
    0.041838173,
    0.017255038,
    -0.05011607,
    0.03330435,
    -0.043115288,
    0.024365269,
    0.008319518,
    -0.0010633086,
    -0.029032322,
    -0.03335746,
    0.003915449,
    0.0026979458,
    0.006955503,
    -0.010099175,
    0.026191471,
    -0.041106544,
    -0.0023986483,
    -0.022852676,
    -0.02046144,
    -0.002146331,
    0.01218532,
    -0.0072695855,
    -0.030294996,
    -0.045752205,
    0.032810956,
    -0.0067101414,
    -0.027104286,
    -0.064636745,
    0.004402361,
    0.043752763,
    0.085650235,
    -0.015107115,
    0.022542398,
    -0.012031321,
    -0.016969888,
    0.0076060123,
    -0.0017969686,
    0.0015017944,
    -0.031243084,
    0.021503126,
    -0.031438302,
    0.024717737,
    -0.0144598475,
    0.033105273,
    0.06400776,
    0.0064835474,
    0.020815318,
    -0.035197426,
    0.008032621,
    -0.033077635,
    -0.0075795515,
    0.0020131955,
    -0.04924523,
    -0.015111905,
    0.09664927,
    0.039005585,
    0.018401628,
    -0.008501611,
    -0.04901847,
    -0.018197248,
    0.010896852,
    0.025534352,
    0.007962412,
    -0.06537132,
    0.047222693,
    0.047002513,
    -0.032187644,
    0.061376147,
    0.01863283,
    0.08137537,
    -0.024960617,
    -0.012657353,
    0.0015068561,
    -0.041729964,
    0.048218288,
    -0.017299946,
    0.032438353,
    0.018043075,
    0.022542244,
    0.033174258,
    -0.053141434,
    -0.0022261054,
    0.0031429217,
    -0.038417198,
    0.026048293,
    -0.008870415,
    0.030812439,
    0.03319375,
    0.011411405,
    0.040851586,
    0.046423644,
    -0.043405082,
    -0.04912621,
    0.031803377,
    -0.016694633,
    0.033403166,
    0.030523121,
    0.011753323,
    0.023821648,
    0.05829599,
    0.017138349,
    0.026108429,
    0.02419801,
    -0.0020035687,
    -0.010595497,
    -0.02986105,
    -0.012447884,
    0.053139225,
    -0.0010849425,
    -0.0024957422,
    0.023570115,
    0.01298907,
    -0.04547553,
    0.0389707,
    -0.046143718,
    -0.019500313,
    0.0116330525,
    -0.00965437,
    0.038469248,
    0.048517454,
    0.02178535,
    -0.052851528,
    -0.02059403,
    0.0143086715,
    -0.045374274,
    0.047701433,
    -0.016601518,
    0.037074994,
    -0.04640927,
    -0.0002305248,
    0.033905182,
    0.039176434,
    -0.028918877,
    0.001103291,
    -0.061383422,
    0.013512757,
    -0.00791641,
    -0.029770156,
    -0.024654103,
    -0.015810521,
    0.0084724855,
    -0.033417154,
    -0.030438572,
    0.010559214,
    -0.0012923476,
    0.022190606,
    0.060173254,
    -0.0071199643,
    0.009328146,
    0.05591182,
    0.0048989854,
    -0.00032818865,
    -0.0060883774,
    -0.00023993742,
    0.036252055,
    0.015170894,
    -0.010240388,
    -0.010290683,
    -0.011864539,
    -0.009298821,
    -0.051631484,
    0.004371834,
    0.0011985721,
    0.027394142,
    0.018983513,
    0.047578435,
    0.0006170196,
    0.022756878,
    -0.047732078,
    0.0074887024,
    -0.04091164,
    0.007259588,
    -0.004065235,
    -0.031365845,
    0.013086743,
    -0.012649365,
    0.028764397,
    -0.0018775607,
    0.021444034,
    -0.018342517,
    0.012937729,
    0.015516735,
    -0.013029383,
    0.015664442,
    -0.024555063,
    0.00019338762,
    0.0017034123,
    0.0004926277,
    0.009059934,
    -0.014520103,
    0.05243122,
    0.014932085,
    -0.0039938837,
    0.019125547,
    -0.008614347,
    -0.06729333,
    -0.013556678,
    0.05028874,
    -0.0018153647,
    0.015557867,
    -0.0030649423,
    -0.0026829718,
    0.018642286,
    -0.038199183,
    -0.03294994,
    -0.014883622,
    0.014779545,
    0.02140016,
    0.009804876,
    0.004958428,
    0.019226104,
    0.025099836,
    0.046242405,
    0.0008196466,
    0.018209888,
    -0.02076707,
    0.059340294,
    -0.031871144,
    -0.037058495,
    -0.0046318094,
    -0.012318178,
    0.011814306,
    0.0041106166,
    -0.016442508,
    0.002910965,
    -0.010647634,
    -0.008500043,
    0.013334221,
    0.020931307,
    -0.014139455,
    -0.030637203,
    0.004125956,
    0.0312838,
    -0.039864857,
    0.030869365,
    -0.016873274,
    0.0056212116,
    -0.013738663,
    0.004012051,
    -0.038413186,
    -0.028748166,
    -0.024072843,
    0.057576973,
    0.017201373,
    0.028801078,
    -0.009352578,
    0.005576139,
    0.010144287,
    -0.05617081,
    0.026736649,
    -0.057129078,
    -0.037356164,
    0.04270804,
    -0.022015018,
    0.025703205,
    0.016018357,
    0.004235701,
    -0.001066849,
    -0.0133604165,
    0.0039634574,
    -0.0009934092,
    -0.04011141,
    -0.009605451,
    -0.042391464,
    0.029926252,
    -0.0022060736,
    -0.06582467,
    0.03539945,
    0.031970825,
    -0.015887093,
    -0.010586142,
    0.0025160008,
    0.027151367,
    0.015396707,
    0.020803122,
    -0.012347851,
    0.041142147,
    0.01460739,
    -0.027189141,
    -0.0084227305,
    0.03268739,
    0.03432998,
    -0.050671257,
    -0.006849337,
    0.05580775,
    -0.029546585,
    -0.19109386,
    0.008132767,
    -0.00625366,
    0.008462262,
    -0.005741844,
    0.027879208,
    -0.04825245,
    0.0048290244,
    0.0030262228,
    -0.012869358,
    -0.010487197,
    -0.033437826,
    0.00086632045,
    -0.0031849043,
    0.054632913,
    0.012125366,
    -0.0034956357,
    -0.023784228,
    0.0045979237,
    -0.06838102,
    0.0066340277,
    0.008821881,
    -0.017112399,
    -0.06651932,
    0.016837852,
    0.016893044,
    0.014203568,
    -0.010174751,
    -0.029387718,
    -0.011306487,
    -0.027990853,
    -0.0028507991,
    0.012847916,
    0.030015633,
    0.061893035,
    0.040559474,
    0.06450448,
    0.008577098,
    0.01361189,
    0.01301374,
    0.017445505,
    0.063280314,
    -0.024008118,
    -0.0410387,
    0.009988834,
    -0.004833229,
    0.0031237896,
    0.012673825,
    -0.032089576,
    -0.020773202,
    -0.018866468,
    -0.0030336387,
    -0.037033644,
    0.02092163,
    0.002071078,
    -0.015567679,
    -0.033961352,
    0.032231517,
    -0.037392493,
    -0.020856244,
    -0.030775473,
    -0.03454945,
    0.004895689,
    0.016605146,
    -0.055688687,
    0.013458171,
    -0.020007674,
    -0.028545652,
    -0.008191386,
    -0.011002774,
    0.050427735,
    -0.008550305,
    0.0118111,
    -0.005803428,
    -0.026859796,
    -0.011692541,
    0.021300903,
    -0.028170336,
    -0.017763572,
    -0.13710505,
    0.004965118,
    -0.012338429,
    -0.009626636,
    0.033704028,
    0.007601361,
    -0.044706993,
    0.063490316,
    0.015604505,
    0.031396233,
    0.24593687,
    -0.034070414,
    0.023450267,
    -0.030969962,
    0.038910042,
    -0.023677358,
    0.0071090786,
    -0.011207256,
    0.029248567,
    -0.04609916,
    -0.022783192,
    -0.014655579,
    0.0013965754,
    -0.0036873475,
    -0.019272102,
    0.011954131,
    -0.040581945,
    0.010395461,
    0.070001654,
    0.028521886,
    0.020681182,
    -0.010727249,
    0.024728553,
    -0.0018973184,
    -0.016035778,
    -0.04022159,
    0.015369633,
    0.053623963,
    -0.0032370207,
    -0.0068921903,
    -0.0074646845,
    -0.045909774,
    0.024136009,
    -0.012132545,
    -0.02143451,
    -0.009162377,
    -0.010898247,
    -0.031385545,
    0.011661473,
    -0.012991721,
    -0.010576877,
    0.011779889,
    0.006928308,
    0.025649205,
    0.0028401532,
    0.015434813,
    -0.031618256,
    -0.020008171,
    -0.035858158,
    0.0009007813,
    -0.010263004,
    -0.02045078,
    -0.060780726,
    0.02870223,
    -0.059399962,
    -0.02819086,
    -0.028941907,
    0.0014574742,
    0.018966153,
    0.059438027,
    0.00813851,
    0.041569088,
    -0.04852137,
    -0.025426703,
    -0.04566685,
    0.0013227283,
    -0.0135409115,
    -0.021306759,
    -0.016258981,
    0.01099489,
    0.011348335,
    -0.029114893,
    0.00058327557,
    0.026428098,
    -0.0037051656,
    0.012885822,
    -0.029917996,
    0.030765334,
    -0.005484935,
    0.0053331107,
    -0.025947286,
    -0.039691433,
    -0.014631929,
    -0.009714047,
    0.014868744,
    -0.013864954,
    -0.030055424,
    0.01786473,
    -0.0092636915,
    -0.0109823365,
    0.056882,
    0.009323296,
    0.0037069088,
    0.004796603,
    -0.0048888833,
    0.0054285945,
    0.043755546,
    0.024507822,
    0.025022179,
    0.027026204,
    -0.08134872,
    -0.012025706,
    0.02460811,
    -0.013556706,
    0.026682822,
    -0.011773854,
    0.016998423,
    -9.3735e-5,
    -0.032791283,
    -0.009831742,
    0.053448338,
    -0.004855143,
    0.0069636162,
    0.020332327,
    0.039362658,
    0.036531907,
    -0.006381021,
    -5.527525e-6,
    -0.01604043,
    0.06029084,
    -0.05366821,
    -0.024639117,
    -0.0060600154,
    -0.008861102,
    -0.0045871404,
    0.008669352,
    -0.06810332,
    0.0018733272,
    0.018493325,
    0.017002486,
    -0.029507855,
    -0.037704434,
    -0.01631373,
    0.08775386,
    0.04600553,
    -0.04080889,
    0.07545939,
    0.019134983,
    -0.032352936,
    0.058893166,
    -0.02953855,
    -0.03984061,
    -0.012755565,
    0.0014477421,
    -0.029224813,
    0.054907944,
    -0.0789144,
    0.002413634,
    -0.0051396578,
    0.051368546,
    -0.007456196,
    -0.0057195937,
    0.052404836,
    -0.05682206,
    -0.030991841,
    0.006827349,
    0.003521702,
    0.017826024,
    -0.020567209,
    -0.027690174,
    0.01883157,
    -0.0074440874,
    0.053265754,
    0.09342776,
    0.027881276,
    0.029499996,
    -0.015187565,
    0.05059695,
    -0.013954103,
    -0.03284258,
    -0.004100567,
    -0.036653206,
    -0.024409015,
    -0.019542146,
    -0.011304147,
    -0.004688139,
    -0.057332404,
    -0.0027535206,
    -0.02539958,
    0.025160607,
    0.038703024,
    -0.02674856,
    -0.017489722,
    -0.002494743,
    0.008934229,
    0.048612032,
    0.0049296618,
    -0.0064484146,
    0.042560503,
    -0.0066472767,
    -0.0013230841,
    0.07318776,
    0.002059235,
    -0.010504023,
    0.020186918,
    0.022652715,
    0.028194541,
    0.022320177,
    0.02590463,
    -0.007175373,
    -0.007648733,
    -0.036022216,
    -0.0031242715,
    -0.009156579,
    -0.010659548,
    0.008049303,
    0.008840813,
    0.02352207,
    0.0017198211,
    0.003525938,
    -0.017763577,
    -0.02255104,
    0.0054182066,
    0.0027917984,
    -0.030119449,
    0.015834024,
    0.015099323,
    0.0032004844,
    0.0024566595,
    -0.050682098,
    -0.0022582116,
    -0.0037904717,
    0.045005098,
    -0.011423952,
    0.0067611965,
    -0.030309727,
    0.019692667,
    0.032845058,
    -0.0090010865,
    -0.01480977,
    0.0005478675,
    0.008241499,
    -0.018594833,
    0.020048302,
    -0.003415002,
    0.022371223,
    -0.044811677,
    0.014281272,
    0.014886089,
    -0.026090553,
    0.002907364,
    0.01371469,
    0.0092705805,
    0.04732476,
    -0.012872408,
    0.05785681,
    -0.02855162,
    -0.024949966,
    -0.0375568,
    0.0020091098,
    -0.037340682,
    -0.009061861,
    0.03339302,
    -0.025103046,
    0.046012443,
    -0.020558462,
    0.028964512,
    -0.006917054,
    -0.0770982,
    0.01828087,
    -0.024794715,
    0.01697373,
    -0.025829177,
    -0.034757238,
    -0.03368985,
    -0.03379701,
    0.040056404,
    0.004607489,
    -0.0218689,
    -0.050506763,
    0.014846354,
    -0.020619864,
    -0.02638047,
    -0.010243197,
    -0.019768784,
    0.0037510414,
    -0.0075338874,
    0.01765253,
    -0.02485942,
    0.011276767,
    -0.022816496,
    0.0045660967,
    -0.018123796,
    0.04748066,
    -0.0072258823,
    0.039989892,
    -0.022220366,
    0.014220618,
    0.009477211,
    -0.039026357,
    0.0062931096,
    -0.018500213,
    -0.060098987,
    -0.011408045,
    -0.024998842,
    -0.0058286777,
    -0.0195909,
    0.020962415,
    -0.03363868,
    0.027674908,
    -0.022782326,
    0.00011843879,
    -0.004896305,
    -0.0036817114,
    0.028065119,
    0.011832172,
    0.0045515657,
    -0.033082232,
    0.07098138,
    0.0034055999,
    0.022488177,
    -0.059109736,
    -0.006545849,
    0.01567799,
    0.045923635,
    -0.040564496,
    0.020701755,
    -0.013943637,
    0.010593306,
    0.03258394,
    0.022522068,
    -0.0010788155,
    0.0076529128,
    0.05484559,
    -0.010061054,
    0.009654935,
    -0.0022583394,
    0.05352336,
    -0.011079317,
    0.0035695934,
    -0.028402412,
    -0.006438774,
    -0.06544066,
    0.0015482869,
    -0.02509151,
    0.0032823705,
    0.07090053,
    0.0056362124,
    0.039916538,
    -0.025373423,
    -0.041575126,
    0.060639337,
    0.0029663588,
    -0.012050132,
    -0.027709965,
    -0.02914558,
    0.019477332,
    0.011386469,
    0.011246975,
    -0.036803737,
    -5.930105e-5,
    0.054610252,
    -0.0067799045,
    -0.014393941,
    0.022297248,
    0.0032388605,
    -0.013143182,
    0.037334643,
    0.02809063,
    0.0024890795,
    -0.0098310355,
    0.022139102,
    -0.000509493,
    0.026036005,
    -0.01182871,
    0.0012622843,
    -0.03270492,
    0.017757278,
    -0.035536855,
    -0.072909854,
    -0.03651895,
    0.0040604817,
    -0.016163552,
    0.017194662,
    0.02697014,
    0.042724665,
    0.023468137,
    0.019033179,
    0.043340992,
    -0.016657673,
    -0.002251577,
    -0.01508343,
    -0.02167459,
    -0.051993024,
    -0.0059517934,
    -0.06468132,
    -0.057369076,
    -0.005457933,
    0.040747315,
    0.013892949,
    -0.054217882,
    -0.0067164046,
    -0.018244999,
    0.016202413,
    -0.17906684,
    0.0077831107,
    -0.016322823,
    -0.022605948,
    -0.0341306,
    -1.4258145e-5,
    -0.024862673,
    -0.026506605,
    -0.018937126,
    -0.0015299825,
    -0.02946169,
    0.015510216,
    -0.029751161,
    -0.03022711,
    -0.036712512,
    0.031198155,
    0.013417892,
    -0.008315913,
    0.008630413,
    0.05939868,
    0.004666387,
    -0.03700047,
    0.01061426,
    -0.04444638,
    0.0011062768,
    -0.01687693,
    -0.013345257,
    0.011018251,
    -0.045670312,
    -0.055196635,
    0.02702817,
    -0.0012713602,
    0.016427027,
    0.065190285,
    -0.05031679,
    0.0354162,
    0.0111456355,
    0.019916313,
    0.01477492,
    0.006546737,
    0.005281983,
    0.019113593,
    -0.030623721,
    -0.007362806,
    -0.0048263073,
    -0.00022648936,
    -0.012456244,
    -0.026633704,
    -0.059150986,
    -0.024084214,
    0.03341897,
    -0.004672881,
    -0.0032702186,
    0.02231938,
    0.026500896,
    0.0231716,
    -0.040338017,
    0.015829084,
    0.00064458227,
    0.03079693,
    -0.0073098433,
    0.004672375,
    -0.04269056,
    -0.034884535,
    -0.03460636,
    0.042699225,
    -0.070788555,
    -0.0042571486,
    -0.011707434,
    0.026174184,
    -0.040012766,
    -0.014723488,
    0.03398638,
    -0.02656347,
    0.055813666,
    0.0022373649,
    0.027270485,
    -0.008574889,
    -0.047244847,
    -0.020595375,
    -0.0024405264,
    -0.015287482,
    -0.034606777,
    0.014166236,
    0.014883017,
    -0.024907274,
    0.0036789668,
    0.04588317,
    -0.025727632,
    0.007474228,
    -0.041889444,
    -0.04239232,
    -0.031765148,
    0.016402654,
    -0.032313958,
    -0.042583466,
    0.003694188,
    -0.03789163,
    0.005762771,
    -0.014236267,
    0.026926348,
    0.0145796435,
    -0.046388373,
    0.010923083,
    0.004996239,
    0.062479153,
    0.025659053,
    -0.02194447,
    0.005160734,
    -0.025858726,
    -0.03657522,
    0.011269099,
    0.020505859,
    0.02050172,
    0.028689394,
    -0.032361012,
    0.026581394,
    -0.006538726,
    -0.02164772,
    -0.02820093,
    7.197924e-6,
    0.006236892,
    0.035982244,
    -0.029548632,
    0.059329294,
    -0.019903114,
    0.030476535,
    0.0012749213,
    -0.0067717233,
    -0.057012323,
    0.05123047,
    -0.022876687,
    0.007296464,
    -0.0058410163,
    -0.012961809,
    -0.022470405,
    0.022805417,
    0.027031465,
    -0.047690865,
    -0.0377045,
    0.033635926,
    -0.037884004,
    -0.036368813,
    -0.008691378,
    -0.011877837,
    0.027587203,
    0.03739567,
    -0.010263341,
    -0.016878022,
    -0.017726379,
    -0.0028035117,
    0.016174102,
    0.007416928,
    0.016449932,
    -0.044825092,
    0.028005298,
    0.0075571584,
    -0.0045754467,
    0.02552638,
    -0.017322907,
    -0.054073393,
    -0.0022051185,
    0.016951907,
    -0.00097456406,
    -0.0057038623,
    0.005191519,
    0.009454499,
    -0.017367927,
    -0.031111648,
    -0.017883712,
    0.0061059613,
    0.03894756,
    -0.014612449,
    0.021154026,
    0.041503686,
    -0.00025324986,
    0.041500125,
    -0.01261671,
    0.035225008,
    -0.021307211,
    0.004774336,
    0.009435292,
    -0.0037574295,
    0.027970085,
    -0.010632901,
    0.020761402,
    0.028760817,
    0.0014389881,
    0.047640633,
    0.012061617,
    0.025732249,
    0.0034775035,
    0.017368317,
    -0.0110013895,
    0.048862655,
    -0.00082114513,
    -0.021151956,
    -0.0035007775,
    -0.047433738,
    0.027765855,
    -0.035673257,
    -0.015826378,
    0.015195975,
    -0.03630748,
    0.017007241,
    0.029029569,
    -0.033839382,
    -0.00847942,
    -0.03248065,
    0.066325404,
    -0.031397443,
    0.011676608,
    -0.008554638,
    0.008910565,
    -0.028092973,
    0.006312944,
    -0.0009780206,
    0.019365432,
    0.028579503,
    0.049160477,
    0.020700263,
    0.0059294514,
    -0.0036679148,
    0.00886464,
    0.027618295,
    0.0013910793,
    -0.037796766,
    0.030503033,
    0.0006737808,
    -0.017314281,
    0.025290234,
    -0.05493075,
    -0.026802655,
    0.035179928,
    -0.0026953951,
    -0.049771644,
    0.02076164,
    -0.007511784,
    -0.0048586307,
    -0.051234838,
    0.036292616,
    0.028930582,
    -0.017055722,
    0.016079217,
];

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

async fn prepare_test_shop() -> Shop {
    let stack = get_cfn_output();
    let shop = Faker.fake::<Shop>();
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut shop_records = ShopRecord::clone_from_shop_as_shop_domain_records(&shop);
    shop_records.push(ShopRecord::from_shop_as_shop_id_record(shop.clone()));
    dynamodb_repository
        .put_shop_records_transact(shop_records)
        .await
        .unwrap();
    shop
}

async fn refresh_index(index: &str) {
    get_opensearch_client()
        .await
        .indices()
        .refresh(IndicesRefreshParts::Index(&[index]))
        .send()
        .await
        .unwrap()
        .error_for_status_code()
        .unwrap();
}

/// Polls OpenSearch until a document with the given `id` appears in `index`, issuing an explicit
/// index refresh before each attempt. This is necessary because Localstack's OpenSearch requires
/// a refresh before documents become visible — even via direct GET by ID.
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
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(
                put_product_data.shops_product_id,
                materialized.shops_product_id
            );
            assert_eq!(put_product_data.url, materialized.url);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not found in DynamoDB after 60s",
                shop.shop_id, put_product_data.shops_product_id
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

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let materialized = repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap();

        if let Some(materialized) = materialized
            && ProductState::from(new_state) == ProductState::from(materialized.state)
        {
            assert_eq!(shop.shop_id, materialized.shop_id);
            assert_eq!(
                ProductState::from(new_state),
                ProductState::from(materialized.state)
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: ProductRecord for shop '{}' / product '{}' not updated with expected state after 60s",
                shop.shop_id, put_product_data.shops_product_id
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
    materialized_old.text_embedding = None;
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

    let embedding = vec![0.4269f32; 1024];
    let product_event_records = Batch::try_from_iter([ProductEventRecord::from(ProductEvent {
        aggregate_id: materialized_old.product_id,
        event_id: materialized_old.event_id,
        timestamp: OffsetDateTime::now_utc(),
        payload: ProductEventPayload::ProductEnrichmentEvent(
            ProductEnrichmentEventPayload::EmbeddedText(
                EmbeddedTextProductEnrichmentEventPayload {
                    shop_id: materialized_old.shop_id,
                    shops_product_id: materialized_old.shops_product_id.clone(),
                    embedding: embedding.clone(),
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
            && let Some(text_embedding) = materialized.text_embedding
        {
            assert_eq!(embedding, text_embedding);
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

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

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

    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

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
    materialized_old.text_embedding = None;
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
    os_doc.text_embedding = None;
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
            ProductEnrichmentEventPayload::EmbeddedText(
                EmbeddedTextProductEnrichmentEventPayload {
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
            && let Some(text_embedding) = hit.source.text_embedding
        {
            assert_eq!(EXAMPLE_EMBEDDING.as_slice(), &text_embedding);
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

// ---------------------------------------------------------------------------
// Shop ingest flow
// Verifies the shop-opensearch-index Lambda is triggered via EventBridge/SQS
// and has the necessary IAM access to index into OpenSearch.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_create_shop_dynamodb_and_index_opensearch_when_post_shop_then_patch() {
    let stack = get_cfn_output();
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);

    // POST
    let post_url = format!("{}/api/v1/shops", stack.api_gateway_endpoint_url);
    let post_shop_data = Faker.fake::<PostShopData>();
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&post_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let post_res = post_response.json::<GetShopData>().await.unwrap();
    assert!(
        dynamodb_repository
            .get_shop_record_by_id(&post_res.shop_id)
            .await
            .unwrap()
            .is_some()
    );
    let shop_document = wait_for_document::<ShopDocument>("shops", post_res.shop_id).await;
    assert_eq!(post_res.name, shop_document.name);

    // PATCH
    let patch_url = format!(
        "{}/api/v1/shops/{}",
        stack.api_gateway_endpoint_url, post_res.shop_id
    );
    let mut patch_shop_data = Faker.fake::<PatchShopData>();
    patch_shop_data.image = Some(Url::parse("https://rainer.calmund/whopper-happy-meal").unwrap());
    let patch_response = reqwest::Client::new()
        .patch(patch_url)
        .json(&patch_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patch_res = patch_response.json::<GetShopData>().await.unwrap();
    let patched_record = dynamodb_repository
        .get_shop_record_by_id(&patch_res.shop_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        patch_shop_data.image.unwrap(),
        patched_record.image.unwrap()
    );
    let patched_document = wait_for_document::<ShopDocument>("shops", patch_res.shop_id).await;
    assert_eq!(patch_res.name, patched_document.name);
}

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
    let mut put_product_data: PutProductData = Faker.fake();
    put_product_data
        .url
        .set_host(Some(shop.domains.into_iter().next().unwrap().as_str()))
        .unwrap();
    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    tokio::time::sleep(Duration::from_secs(45)).await;

    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    assert!(
        product_repository
            .get_product_record(&shop.shop_id, &put_product_data.shops_product_id)
            .await
            .unwrap()
            .is_some()
    );

    // Create and configure user
    let user = create_test_user(&get_test_mail()).await;
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

    // Add product to watchlist
    let post_url = format!("{}/api/v1/me/watchlist", stack.api_gateway_endpoint_url);
    let post_response = reqwest::Client::new()
        .post(post_url)
        .json(&ProductKeyData {
            shop_id: shop.shop_id,
            shops_product_id: put_product_data.shops_product_id.clone(),
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
        stack.api_gateway_endpoint_url, shop.shop_id, put_product_data.shops_product_id
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
    let patched = patch_response.json::<WatchlistProductData>().await.unwrap();
    tokio::time::sleep(Duration::from_secs(10)).await;

    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );
    let eligible = watchlist_repository
        .query_user_ids_watching_product(&patched.product_id)
        .await
        .unwrap();
    let eligible_user_ids: Vec<UserId> = eligible.into_iter().map(|(user_id, _)| user_id).collect();
    assert_eq!(vec![UserId::from(user.sub)], eligible_user_ids);
    tokio::time::sleep(Duration::from_secs(10)).await;

    // Update product state to trigger notification
    put_product_data.state = if matches!(put_product_data.state, ProductStateData::Available) {
        ProductStateData::Sold
    } else {
        ProductStateData::Available
    };
    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    assert!(wait_for_email("Antiquitäten-Update").await);
}

// ---------------------------------------------------------------------------
// Search filter: OpenSearch sync
// Verifies EventBridge → SQS → Lambda → OpenSearch routing and IAM access
// for create, update, and delete operations on user search filters.
// ---------------------------------------------------------------------------

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

// ---------------------------------------------------------------------------
// Search filter percolation
// Verifies that newly ingested products are matched against stored search
// filters and that a notification email is sent to the filter owner.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_send_email_to_user_when_product_matches_search_filter() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // Create and configure user
    let user = create_test_user(&get_test_mail()).await;
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
    let url = format!("{}/api/v1/products", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .put(url)
        .json(&PutCollectionData {
            items: vec![put_product_data.clone()],
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    assert!(wait_for_email("Neues Ergebnis für").await);
}

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
async fn should_respond_200_personalized_when_authenticated_and_product_does_exist_and_watched() {
    let user = create_random_test_user().await;
    let ddb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(ddb_client, &get_cfn_output().dynamodb_table_1_name);
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        ddb_client,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let get_product_service = GetProductServiceImpl::new(&product_repository);
    let watchlist_service = ProductWatchListServiceImpl::new(
        &watchlist_repository,
        &product_repository,
        &get_product_service,
    );

    let record = Faker.fake::<ProductRecord>();
    let insert_res = product_repository
        .put_product_records([record.clone()].into())
        .await
        .unwrap();
    assert!(insert_res.unprocessed_items.unwrap().is_empty());
    tokio::time::sleep(Duration::from_secs(1)).await;
    watchlist_service
        .create_watchlist_product(&user.sub.into(), &record.shop_id, &record.shops_product_id)
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/shops/{}/products/{}?currency=GBP",
        get_cfn_output().api_gateway_endpoint_url,
        record.shop_id,
        record.shops_product_id
    );
    let response = reqwest::Client::new()
        .get(url)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    assert_eq!(record.shop_id.to_string(), body["item"]["shopId"]);
    assert_eq!(record.product_id.to_string(), body["item"]["productId"]);
    assert_eq!(
        record.price_gbp.unwrap(),
        body["item"]["price"]["offer"]["amount"]
    );
    assert_eq!("GBP", body["item"]["price"]["offer"]["currency"]);
    assert!(
        body["userState"]["watchlist"]["watching"]
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
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::PriceDropped(
            ProductPriceChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                new_native_price: event_1_price,
                new_other_price: FixedFxRate()
                    .exchange_all(event_1_price.currency, event_1_price.monetary_amount)
                    .unwrap(),
                old_native_price: Price {
                    monetary_amount: 100000u64.into(),
                    currency: Currency::Eur,
                },
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
        payload: ProductEventPayload::ProductDomainEvent(ProductDomainEventPayload::StateRemoved(
            ProductStateChangeDomainEventPayload {
                shop_id: record.shop_id,
                shops_product_id: record.shops_product_id.clone(),
                old_state: ProductState::Sold,
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
    assert_eq!("PRICE_DROPPED", history[0]["eventType"]);
    assert_eq!("USD", history[0]["payload"]["newPrice"]["currency"]);
    assert_eq!(event_2_id.to_string(), history[1]["eventId"]);
    assert_eq!("STATE_REMOVED", history[1]["eventType"]);
}

// ---------------------------------------------------------------------------
// API: Product search
// Verifies OpenSearch query routing, watchlist personalization for auth users,
// and correct currency/language serialization.
// ---------------------------------------------------------------------------

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
        &get_product_service,
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
        text_embedding: None,
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
        text_embedding: Some(fake::vec![f32; 1024]),
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
        text_embedding: None,
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

// ---------------------------------------------------------------------------
// API: Product similar
// Verifies the ANN/KNN endpoint, 202 when embeddings are missing, and
// watchlist personalization for authenticated users.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_202_when_similar_products_embedding_not_computed() {
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut product_record: ProductRecord = Faker.fake();
    product_record.text_embedding = None;
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
        doc.text_embedding = None;
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

#[localstack_test(services = [Cloudformation()])]
async fn should_respond_200_when_similar_products_computed_for_anon() {
    let product_dynamodb_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let mut product_record: ProductRecord = Faker.fake();
    product_record.text_embedding = Some(EXAMPLE_EMBEDDING.into());
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
        doc.text_embedding = Some(EXAMPLE_EMBEDDING.into());
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
        &get_product_service,
    );

    let mut product_record: ProductRecord = Faker.fake();
    product_record.text_embedding = Some(EXAMPLE_EMBEDDING.into());
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
            .create_watchlist_product(&user_id, &pr.shop_id, &pr.shops_product_id)
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
        doc.text_embedding = Some(EXAMPLE_EMBEDDING.into());
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
        .json::<TimeCursoredData<WatchlistProductDataView>>()
        .await
        .unwrap();
    assert_eq!(1, gotten.items.len());
    assert_eq!(
        &materialized.product_id,
        &gotten.items[0].product.product_id
    );
    assert_eq!(&materialized.shop_id, &gotten.items[0].product.shop_id);
    assert_eq!(
        &materialized.shops_product_id,
        &gotten.items[0].product.shops_product_id
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
            notifications: Some(!gotten.items[0].notifications),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let patch_res = patch_response.json::<WatchlistProductData>().await.unwrap();
    assert_eq!(gotten.items[0].created, patch_res.created);
    assert_eq!(materialized.shop_id, patch_res.shop_id);
    assert_eq!(materialized.shops_product_id, patch_res.shops_product_id);
    assert_eq!(materialized.product_id, patch_res.product_id);

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
        .json::<TimeCursoredData<WatchlistProductDataView>>()
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
    let service = UserSearchFilterServiceImpl::new(&repository);

    let user = create_random_test_user().await;
    let expected1 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected1_name = Faker.fake::<UserSearchFilterName>();
    let expected2 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected2_name = Faker.fake::<UserSearchFilterName>();
    service
        .save_user_search_filter(&user.sub.into(), expected1_name.clone(), expected1.clone())
        .await
        .unwrap();
    service
        .save_user_search_filter(&user.sub.into(), expected2_name.clone(), expected2.clone())
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
            shop_type_query: None,
            price_query: None,
            state_query: None,
            origin_year_query: None,
            authenticity_query: None,
            condition_query: None,
            provenance_query: None,
            restoration_query: None,
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

// ---------------------------------------------------------------------------
// API: Shop
// Verifies API Gateway routing and Lambda IAM access for shop CRUD and
// OpenSearch-backed shop search.
// ---------------------------------------------------------------------------

#[localstack_test(services = [Cloudformation()])]
async fn should_create_update_get_shop() {
    let post_shop_data = PostShopData {
        name: "Woobl woop".into(),
        shop_type: Faker.fake(),
        domains: [Domain::try_from("https://hans-shopping-nig.com").unwrap()].into(),
        image: None,
    };
    let post_url = format!("{}/api/v1/shops", get_cfn_output().api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .post(post_url)
        .json(&post_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, response.status());
    let created = response.json::<GetShopData>().await.unwrap();
    tokio::time::sleep(Duration::from_secs(1)).await;

    let patch_shop_data = PatchShopData {
        shop_type: Faker.fake(),
        domains: None,
        image: Some(Url::parse("https://hans-shopping-nig.co.uk").unwrap()),
    };
    let patch_url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.shop_id
    );
    let response = reqwest::Client::new()
        .patch(patch_url)
        .json(&patch_shop_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let updated = response.json::<GetShopData>().await.unwrap();
    assert_eq!(post_shop_data.domains, updated.domains);
    assert_eq!(
        patch_shop_data.image.unwrap(),
        updated.image.clone().unwrap()
    );
    tokio::time::sleep(Duration::from_secs(1)).await;

    // GET by ID
    let get_url = format!(
        "{}/api/v1/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.shop_id
    );
    let response = reqwest::Client::new().get(get_url).send().await.unwrap();
    assert_eq!(200, response.status());
    let gotten = response.json::<GetShopData>().await.unwrap();
    assert_eq!(updated.shop_id, gotten.shop_id);
    assert_eq!(updated.name, gotten.name);
    assert_eq!(updated.domains, gotten.domains);
    assert_eq!(updated.image, gotten.image);
    assert_eq!(updated.created, gotten.created);

    // GET by slug ID
    let get_slug_url = format!(
        "{}/api/v1/by-slug/shops/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.shop_slug_id
    );
    let response = reqwest::Client::new()
        .get(get_slug_url)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
    let gotten_slug = response.json::<GetShopData>().await.unwrap();
    assert_eq!(updated.shop_id, gotten_slug.shop_id);
    assert_eq!(updated.name, gotten_slug.name);
    assert_eq!(updated.domains, gotten_slug.domains);
    assert_eq!(updated.image, gotten_slug.image);
}

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
    tracing::log::info!("Foo");
    tokio::time::sleep(Duration::from_secs(1200)).await;
    assert_eq!(200, response.status());
}

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
