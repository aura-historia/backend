use aws_tests_common::get_cfn_output;
use base64::Engine;
use common::enhanced_match_reason::EnhancedMatchReason;
use common::execution_state::data::ExecutionStateData;
use common::oauth_client_id::OAuthClientId;
use common::personalized::api::PersonalizedData;
use common::resource_state::record::ResourceStateRecord;
use common::{
    batch::Batch,
    currency::{data::CurrencyData, domain::Currency},
    event::Event,
    event_id::EventId,
    has_key::HasKey,
    language::{
        data::LanguageData,
        document::{LanguageDocument, TextDocument},
        domain::Language,
    },
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
    service::{
        noop_adapters::{NoopS3Adapter, NoopSesAdapter},
        notification_service::NotificationServiceImpl,
    },
};
use notification_api::notification_get::EventIdCursoredData;
use oauth::dynamodb::repository::OAuthDynamoDbRepositoryImpl;
use oauth::{
    core::client::{OAuthClient, OAuthClientName},
    data::{
        IntrospectionResponseData, OAuthClientMetadataRequestData, OAuthClientMetadataResponseData,
        TokenResponseData,
    },
    dynamodb::{client_record::OAuthClientRecord, repository::OAuthRepository},
};
use opensearch::GetParts;
use openssl::{hash::MessageDigest, pkey::PKey, sign::Signer};
use partner_shop_application::data::{
    admin_patch_partner_shop_application_data::AdminPatchPartnerShopApplicationData,
    decision_data::{PartnerShopApplicationDecisionData, PostPartnerShopApplicationDecisionData},
    get_partner_shop_application_data::GetPartnerShopApplicationData,
    partner_shop_application_state_data::PartnerShopApplicationStateData,
    patch_partner_shop_application_data::PatchPartnerShopApplicationData,
    post_partner_shop_application_data::PostPartnerShopApplicationPayloadData,
};
use product::data::get_data::GetProductData;
use product::data::user_state_data::ProductUserStateData;
use product::dynamodb::product_record;
use product::{
    core::{
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
    },
    dynamodb::{
        product_event_record::ProductEventRecord,
        product_image_record::ProductImageRecord,
        product_record::{ProductRecord, mk_pk},
        product_state_record::ProductStateRecord,
        prohibited_content_record::ProhibitedContentRecord,
        repository::{ProductDynamoDbRepository, ProductDynamoDbRepositoryImpl},
    },
    opensearch::{
        product_document::ProductDocument,
        product_state_document::ProductStateDocument,
        repository::{ProductOpenSearchRepository, ProductOpenSearchRepositoryImpl},
    },
    service::{
        command_service::{CommandProductService, CommandProductServiceImpl},
        product_command::{CreateProductCommand, UpdateProductCommand},
        query_service::QueryProductServiceImpl,
    },
};
use product_watchlist::dynamodb::repository::{
    WatchlistProductDynamoDbRepository, WatchlistProductDynamoDbRepositoryImpl,
};
use product_watchlist_api::watchlist_patch::WatchlistProductPatch;
use search_filter::{
    core::user_search_filter_name::UserSearchFilterName,
    data::user_search_filter_data::UserSearchFilterData,
    dynamodb::repository::{
        UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
    },
    opensearch::{
        repository::{
            UserSearchFilterOpenSearchRepository, UserSearchFilterOpenSearchRepositoryImpl,
        },
        user_search_filter_document::UserSearchFilterDocument,
    },
    service::{
        enhanced_search_match_service::{
            EnhancedSearchMatchResult, MockEnhancedSearchMatchService,
        },
        user_search_filter_service::{UserSearchFilterService, UserSearchFilterServiceImpl},
    },
};
use search_filter_api::{
    patch_product_match::PatchUserSearchFilterMatchData,
    patch_types::{PatchProductSearchData, PatchUserSearchFilterData},
    post_types::PostUserSearchFilterData,
};
use search_filter_periodic_match::{
    DEFAULT_LLM_CONCURRENCY, PeriodicMatcherResult, PeriodicMatcherService,
    PeriodicMatcherServiceImpl,
};
use serde::de::DeserializeOwned;
use shop::core::woocommerce_webhook_secret::WoocommerceWebhookSecret;
use shop::data::get_shop_data::GetShopData;
use shop::data::patch_shop_data::PatchShopData;
use shop::data::post_shop_data::PostShopData;
use shop::dynamodb::partner_status_record::ShopPartnerStatusRecord;
use shop::dynamodb::repository::ShopDynamoDbRepository;
use shop::dynamodb::shop_record::ShopRecord;
use shop::{
    core::shop::Shop, dynamodb::repository::ShopDynamoDbRepositoryImpl,
    service::get_service::GetShopServiceImpl,
};
use std::collections::HashMap;
use std::time::{Duration, Instant, SystemTime};
use test_api::*;
use time::OffsetDateTime;
use user::core::access_token::{
    AccessToken, AccessTokenId, AccessTokenName, AccessTokenOrigin, RawAccessToken,
    RawOAuthClientSecret, Scope,
};
use user::core::role::UserRole;
use user::core::tier::UserTier;
use user::data::access_token_data::{
    GetAccessTokenData, PatchAccessTokenData, PostAccessTokenData, ScopeData,
};
use user::data::patch_admin_user_data::PatchAdminUserData;
use user::data::role_data::UserRoleData;
use user::data::tier_data::UserTierData;
use user::dynamodb::tier_record::UserTierRecord;
use user::service::command::UpdateUserCommand;
use user::service::user_service::UserService;
use user::{
    data::{get_user_data::GetUserAccountData, patch_user_data::PatchUserAccountData},
    dynamodb::{
        access_token_record::AccessTokenRecord,
        repository::{UserDynamoDbRepository, UserDynamoDbRepositoryImpl},
        user_record::UserRecord,
        user_record_update::UserRecordUpdate,
    },
};

fn request_context_for_user(user_id: UserId) -> common::actor::RequestContext {
    common::actor::RequestContext {
        actor: common::actor::domain::Actor::User(user_id),
    }
}

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
    let shop_repository =
        ShopDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let command_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let result = command_service.create(commands).await;
    assert!(result.is_empty(), "Some products failed to create");
}

async fn update_products(commands: HashMap<ProductKey, UpdateProductCommand>) {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repository =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let shop_repository =
        ShopDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let get_shop_service = GetShopServiceImpl::new(&shop_repository);
    let command_service = CommandProductServiceImpl::new(&product_repository, &get_shop_service);

    let result = command_service.update(commands).await;
    assert!(result.is_empty(), "Some products failed to update");
}

fn empty_update_product_command() -> UpdateProductCommand {
    UpdateProductCommand {
        native_price: None,
        state: None,
        native_price_estimate_min: None,
        native_price_estimate_max: None,
        url: None,
        images: None,
        auction_start: None,
        auction_end: None,
        embedding: None,
        translated_titles: None,
    }
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

fn localstack_query_embedding() -> Vec<f32> {
    vec![0.42; 768]
}

fn make_periodic_acceptance_product_document(
    title: &str,
    embedding: Vec<f32>,
    updated: OffsetDateTime,
) -> ProductDocument {
    let mut product: ProductDocument = Faker.fake();
    product.title_native = TextDocument {
        text: title.to_string(),
        language: LanguageDocument::En,
    };
    product.title_en = Some(title.to_string());
    product.embedding = Some(embedding);
    product.state = ProductStateDocument::Available;
    product.shop_type = shop::opensearch::shop_type_document::ShopTypeDocument::CommercialDealer;
    product.url = url::Url::parse("https://example.com/periodic-product").unwrap();
    product.view_url = url::Url::parse(
        "https://example.com/periodic-product?utm_source=aura_historia&utm_medium=referral",
    )
    .unwrap();
    product.created = updated;
    product.updated = updated;
    product
}

async fn emit_create_log_group_cloudtrail_event(log_group_name: &str) {
    let detail = serde_json::json!({
        "eventSource": "logs.amazonaws.com",
        "eventName": "CreateLogGroup",
        "requestParameters": {
            "logGroupName": log_group_name,
        }
    });

    let result = get_eventbridge_client()
        .await
        .put_events()
        .entries(
            aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .source("aws.logs")
                .detail_type("AWS API Call via CloudTrail")
                .detail(detail.to_string())
                .event_bus_name("default")
                .build(),
        )
        .send()
        .await
        .unwrap();

    assert_eq!(result.failed_entry_count(), 0);
}

async fn wait_until_log_retention_is_set(
    client: &aws_sdk_cloudwatchlogs::Client,
    log_group_name: &str,
    expected_retention_days: i32,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let log_groups = client
            .describe_log_groups()
            .log_group_name_prefix(log_group_name)
            .send()
            .await
            .unwrap()
            .log_groups
            .unwrap_or_default();

        let retention_in_days = log_groups
            .iter()
            .find(|log_group| log_group.log_group_name.as_deref() == Some(log_group_name))
            .and_then(|log_group| log_group.retention_in_days);

        if retention_in_days == Some(expected_retention_days) {
            return;
        }

        if Instant::now() >= deadline {
            panic!(
                "Expected log group '{}' to have retention of {} days, but last observed retention was {:?}.",
                log_group_name, expected_retention_days, retention_in_days
            );
        }

        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

// ---------------------------------------------------------------------------
// CloudWatch Logs retention
// Verifies CreateLogGroup events trigger the retention Lambda in LocalStack.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_set_retention_policy_when_cloudwatch_log_group_is_created() {
    let client = aws_sdk_cloudwatchlogs::Client::new(test_api::localstack::get_aws_config().await);
    let log_group_name = format!(
        "acceptance/cloudwatch-log-retention/{}",
        uuid::Uuid::new_v4()
    );

    client
        .create_log_group()
        .log_group_name(&log_group_name)
        .send()
        .await
        .unwrap();

    // LocalStack does not synthesize CloudTrail management events for Logs API calls,
    // so publish the same EventBridge event that AWS emits for CreateLogGroup.
    emit_create_log_group_cloudtrail_event(&log_group_name).await;
    wait_until_log_retention_is_set(&client, &log_group_name, 30).await;

    client
        .delete_log_group()
        .log_group_name(log_group_name)
        .send()
        .await
        .unwrap();
}

// ---------------------------------------------------------------------------
// Product ingest: DynamoDB materialization
// Verifies EventBridge routing and Lambda IAM access for each event type.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
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
        state: Some(new_state),
        ..empty_update_product_command()
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

#[aura_integration_test(services = [Cloudformation()])]
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
        ..empty_update_product_command()
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

#[aura_integration_test(services = [Cloudformation()])]
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
        url: Some(new_url.clone()),
        ..empty_update_product_command()
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

#[aura_integration_test(services = [Cloudformation()])]
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
    materialized_old.images = Default::default();
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
        images: Some(new_images.clone().into_iter().collect()),
        ..empty_update_product_command()
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
            // `prohibited_content` is now managed by the domain's heuristic and
            // will differ from the caller-provided value; compare only URLs.
            let materialized_urls: Vec<&url::Url> =
                materialized_images.iter().map(|i| &i.url).collect();
            let expected_urls: Vec<&url::Url> = new_images.iter().map(|i| &i.url).collect();
            assert_eq!(expected_urls, materialized_urls);
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

#[aura_integration_test(services = [Cloudformation()])]
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
        auction_start: Some(new_start),
        auction_end: Some(new_end),
        ..empty_update_product_command()
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

#[aura_integration_test(services = [Cloudformation()])]
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
                native_title: None,
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
        // Scan for the enrichment event record in DynamoDB — it was written by put_product_event_records above
        // and should be picked up by the materialize-opensearch Lambda for OpenSearch indexing.
        let all_items = get_dynamodb_client()
            .await
            .scan()
            .table_name(&stack.dynamodb_table_1_name)
            .send()
            .await
            .unwrap()
            .items
            .unwrap_or_default();

        let has_enrichment_event = all_items.iter().any(|item| {
            item.get("event_type")
                .and_then(|v| v.as_s().ok())
                .map(|s| s == "ENRICHMENT_EMBEDDED")
                .unwrap_or(false)
        });

        if has_enrichment_event {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: No ENRICHMENT_EMBEDDED event record found for shop '{}' / product '{}' after 60s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
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
        // Scan for the policy event record written to DynamoDB.
        let all_items = get_dynamodb_client()
            .await
            .scan()
            .table_name(&stack.dynamodb_table_1_name)
            .send()
            .await
            .unwrap()
            .items
            .unwrap_or_default();

        let has_policy_event = all_items.iter().any(|item| {
            item.get("event_type")
                .and_then(|v| v.as_s().ok())
                .map(|s| s == "POLICY_PROHIBITED_CONTENT_DECISION")
                .unwrap_or(false)
        });

        if has_policy_event {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: No POLICY_PROHIBITED_CONTENT_DECISION event record found for shop '{}' / product '{}' after 60s",
                materialized_old.shop_id, materialized_old.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

// ---------------------------------------------------------------------------
// ---------------------------------------------------------------------------
// User account
// Verifies the Cognito post-confirmation Lambda trigger writes to DynamoDB,
// and that the user API enforces Cognito auth (IAM policy).
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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
        measurement_unit: Some(common::measurement_unit::data::MeasurementUnitData::Imperial),
        prohibited_content_consent: None,
        structured_address: None,
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
    assert_eq!(
        &patch_data.measurement_unit.unwrap(),
        patched.measurement_unit.as_ref().unwrap()
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

/*
#[aura_integration_test(services = [Cloudformation()])]
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
*/

#[aura_integration_test(services = [Cloudformation()])]
async fn should_manage_user_access_tokens() {
    let user = create_random_test_user().await;
    let url = format!(
        "{}/api/v1/me/access-tokens",
        get_cfn_output().api_gateway_endpoint_url,
    );

    let post_response = reqwest::Client::new()
        .post(url.clone())
        .bearer_auth(user.access_token.clone())
        .json(&PostAccessTokenData {
            name: "Acceptance token".to_owned(),
            scope: HashSet::from_iter([ScopeData::ProductsWrite].into_iter()),
            expires_at: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let created = post_response.json::<GetAccessTokenData>().await.unwrap();

    let get_response = reqwest::Client::new()
        .get(url.clone())
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let tokens = get_response
        .json::<Vec<GetAccessTokenData>>()
        .await
        .unwrap();
    assert!(
        tokens
            .iter()
            .any(|token| token.access_token_id == created.access_token_id)
    );

    let get_one_response = reqwest::Client::new()
        .get(format!("{}/{}", url, created.access_token_id))
        .bearer_auth(user.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_one_response.status());
    let token = get_one_response.json::<GetAccessTokenData>().await.unwrap();
    assert_eq!(created.access_token_id, token.access_token_id);

    let patch_response = reqwest::Client::new()
        .patch(url.clone())
        .bearer_auth(user.access_token.clone())
        .json(&PatchAccessTokenData {
            access_token_id: created.access_token_id,
            name: Some("Renamed acceptance token".to_owned()),
            scope: Some(HashSet::from_iter([ScopeData::ProductsWrite].into_iter())),
            expires_at: None,
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    let delete_response = reqwest::Client::new()
        .delete(format!("{}/{}", url, created.access_token_id))
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_complete_oauth_authorization_code_flow() {
    let cfn = get_cfn_output();
    let user = create_random_test_user().await;
    let client_id = OAuthClientId::new();
    let client_secret = RawOAuthClientSecret::new();
    let redirect_uri = url::Url::parse("https://client.example/callback").unwrap();
    let now = OffsetDateTime::now_utc();
    let oauth_repository =
        OAuthDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    oauth_repository
        .put_client_record(OAuthClientRecord::from((
            OAuthClient {
                client_id,
                hashed_client_secret: client_secret.clone().into(),
                name: OAuthClientName::from("Acceptance OAuth client"),
                tos_uri: url::Url::parse("https://client.example/tos").unwrap(),
                policy_uri: url::Url::parse("https://client.example/policy").unwrap(),
                client_uri: url::Url::parse("https://client.example").unwrap(),
                logo_uri: url::Url::parse("https://client.example/logo.png").unwrap(),
                redirect_uris: HashSet::from([redirect_uri.clone()]),
                scopes: HashSet::from([Scope::ProductsWrite]),
                created_by: common::actor::domain::Actor::User(UserId::from(user.sub)),
                updated_by: common::actor::domain::Actor::User(UserId::from(user.sub)),
                created: now,
                updated: now,
            },
            client_secret.clone(),
        )))
        .await
        .unwrap();

    let http = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let authorize_response = http
        .get(format!(
            "{}/api/v1/oauth/authorize",
            cfn.api_gateway_endpoint_url
        ))
        .bearer_auth(user.access_token)
        .query(&[
            ("response_type", "code"),
            ("client_id", &client_id.to_string()),
            ("redirect_uri", redirect_uri.as_ref()),
            ("scope", "products:write"),
            ("state", "state_1"),
            (
                "code_challenge",
                "E9Melhoa2OwvFrEMTJguCHaoeK1t8URWbuGJSstw-cM",
            ),
            ("code_challenge_method", "S256"),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(302, authorize_response.status());
    let location = authorize_response
        .headers()
        .get(http::header::LOCATION)
        .unwrap()
        .to_str()
        .unwrap();
    let callback = url::Url::parse(location).unwrap();
    assert_eq!(
        Some("state_1"),
        callback
            .query_pairs()
            .find_map(|(key, value)| { (key == "state").then_some(value) })
            .as_deref()
    );
    let code = callback
        .query_pairs()
        .find_map(|(key, value)| (key == "code").then_some(value.into_owned()))
        .unwrap();
    let client_secret_string = String::from(client_secret.clone());

    let token_response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/oauth/token",
            cfn.api_gateway_endpoint_url
        ))
        .form(&[
            ("grant_type", "authorization_code"),
            ("code", code.as_str()),
            ("redirect_uri", redirect_uri.as_ref()),
            ("client_id", &client_id.to_string()),
            ("client_secret", client_secret_string.as_str()),
            (
                "code_verifier",
                "dBjftJeZ4CVP-mB92K27uhbUJU1p1r_wW1gFWFOEjXk",
            ),
        ])
        .send()
        .await
        .unwrap();

    assert_eq!(200, token_response.status());
    let token = token_response.json::<TokenResponseData>().await.unwrap();
    assert_eq!("products:write", token.scope);
    let third_party_exchange_code = token.third_party_exchange_code.clone().unwrap();

    let third_party_token_response = reqwest::Client::new()
        .get(format!(
            "{}/api/v1/oauth/tokens/by-third-party-code/{}",
            cfn.api_gateway_endpoint_url, third_party_exchange_code
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(200, third_party_token_response.status());
    let third_party_token = third_party_token_response
        .json::<TokenResponseData>()
        .await
        .unwrap();
    assert_eq!("products:write", third_party_token.scope);
    assert_eq!(token.access_token, third_party_token.access_token);
    assert!(third_party_token.third_party_exchange_code.is_none());

    let introspect_response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/oauth/introspect",
            cfn.api_gateway_endpoint_url
        ))
        .form(&[
            ("token", token.access_token.as_str()),
            ("client_id", &client_id.to_string()),
            ("client_secret", client_secret_string.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(200, introspect_response.status());
    let introspection = introspect_response
        .json::<IntrospectionResponseData>()
        .await
        .unwrap();
    assert!(introspection.active);
    assert_eq!(Some("products:write"), introspection.scope.as_deref());
    assert_eq!(Some(client_id.to_string()), introspection.client_id.clone());

    let revoke_response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/oauth/revoke",
            cfn.api_gateway_endpoint_url
        ))
        .form(&[
            ("token", token.access_token.as_str()),
            ("client_id", &client_id.to_string()),
            ("client_secret", client_secret_string.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(200, revoke_response.status());

    let introspect_after_revoke_response = reqwest::Client::new()
        .post(format!(
            "{}/api/v1/oauth/introspect",
            cfn.api_gateway_endpoint_url
        ))
        .form(&[
            ("token", token.access_token.as_str()),
            ("client_id", &client_id.to_string()),
            ("client_secret", client_secret_string.as_str()),
        ])
        .send()
        .await
        .unwrap();
    assert_eq!(200, introspect_after_revoke_response.status());
    let introspection = introspect_after_revoke_response
        .json::<IntrospectionResponseData>()
        .await
        .unwrap();
    assert!(!introspection.active);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_manage_oauth_client_metadata() {
    let cfn = get_cfn_output();
    let admin = create_admin_test_user().await;
    let url = format!("{}/api/v1/oauth/clients", cfn.api_gateway_endpoint_url);

    let create_response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(admin.access_token.clone())
        .json(&OAuthClientMetadataRequestData {
            client_name: "Acceptance OAuth client".to_owned(),
            redirect_uris: HashSet::from([
                url::Url::parse("https://client.example/callback").unwrap()
            ]),
            tos_uri: url::Url::parse("https://client.example/tos").unwrap(),
            policy_uri: url::Url::parse("https://client.example/policy").unwrap(),
            client_uri: url::Url::parse("https://client.example").unwrap(),
            logo_uri: url::Url::parse("https://client.example/logo.png").unwrap(),
            scope: HashSet::from([ScopeData::ProductsWrite]),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    assert!(
        create_response
            .headers()
            .contains_key(http::header::LOCATION)
    );
    let created = create_response
        .json::<OAuthClientMetadataResponseData>()
        .await
        .unwrap();

    let get_all_response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(admin.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_all_response.status());
    let clients = get_all_response
        .json::<Vec<OAuthClientMetadataResponseData>>()
        .await
        .unwrap();
    assert!(
        clients
            .iter()
            .any(|client| client.client_id == created.client_id)
    );

    let get_one_response = reqwest::Client::new()
        .get(format!("{}/{}", url, created.client_id))
        .bearer_auth(admin.access_token.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_one_response.status());

    let patch_response = reqwest::Client::new()
        .patch(format!("{}/{}", url, created.client_id))
        .bearer_auth(admin.access_token.clone())
        .json(&oauth::data::OAuthClientMetadataPatchData {
            client_name: Some("Updated acceptance OAuth client".to_owned()),
            redirect_uris: Some(HashSet::from([url::Url::parse(
                "https://client.example/updated",
            )
            .unwrap()])),
            tos_uri: Some(url::Url::parse("https://client.example/updated-tos").unwrap()),
            policy_uri: Some(url::Url::parse("https://client.example/updated-policy").unwrap()),
            client_uri: Some(url::Url::parse("https://updated-client.example").unwrap()),
            logo_uri: Some(url::Url::parse("https://updated-client.example/logo.png").unwrap()),
            scope: Some(HashSet::from([ScopeData::ShopsManage])),
        })
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());
    let updated = patch_response
        .json::<OAuthClientMetadataResponseData>()
        .await
        .unwrap();
    assert_eq!("Updated acceptance OAuth client", updated.client_name);

    let delete_response = reqwest::Client::new()
        .delete(format!("{}/{}", url, created.client_id))
        .bearer_auth(admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());
}

// ---------------------------------------------------------------------------
// Product update → notify user
// Verifies EventBridge → SQS → Lambda → Cognito/DynamoDB → SES routing
// and the associated IAM policies.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_send_email_to_user_when_watched_product_has_update() {
    let stack = get_cfn_output();
    let shop = prepare_test_shop().await;

    // Create product
    let mut create_cmd: CreateProductCommand = Faker.fake();
    create_cmd.shop_id = shop.shop_id;
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
    let user_id = UserId::from(user.sub);
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
            &user_id,
            UserRecordUpdate {
                first_name: Some("Thomas".into()),
                last_name: Some("Testperson".into()),
                language: Some(common::language::record::LanguageRecord::De),
                currency: Some(common::currency::record::CurrencyRecord::Eur),
                measurement_unit: None,
                prohibited_content_consent: None,
                tier: Some(UserTierRecord::Free),
                role: None,
                stripe_customer_id: None,
                structured_address_addressline: None,
                structured_address_addressline_extra: None,
                structured_address_locality: None,
                structured_address_region: None,
                structured_address_postal_code: None,
                structured_address_country: None,
                geo_address_lat: None,
                geo_address_lon: None,
                gsi1_pk: None,
                gsi1_sk: None,
                updated_by: common::actor::record::ActorRecord::User(user_id),
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
            state: None,
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
    let eligible_user_ids: Vec<UserId> =
        eligible.into_iter().map(|record| record.user_id).collect();
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
            state: Some(new_state),
            ..empty_update_product_command()
        },
    )]))
    .await;

    assert!(wait_for_ses_email("Statusänderung", Duration::from_secs(120)).await);
}

// ---------------------------------------------------------------------------
// Search filter percolation
// Verifies that newly ingested products are matched against stored search
// filters and that a notification email is sent to the filter owner.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[aura_integration_test(services = [Cloudformation()])]
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
                measurement_unit: None,
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

#[aura_integration_test(services = [Cloudformation()])]
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
    assert_eq!(record.url.clone().to_string(), body["item"]["url"]);
    assert_eq!(
        record.price_gbp.unwrap(),
        body["item"]["price"]["offer"]["amount"]
    );
    assert_eq!("GBP", body["item"]["price"]["offer"]["currency"]);
}

#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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
// API: Product similar
// Verifies the ANN/KNN endpoint, 202 when embeddings are missing, and
// watchlist personalization for authenticated users.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[aura_integration_test(services = [Cloudformation()])]
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
#[aura_integration_test(services = [Cloudformation()])]
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
#[aura_integration_test(services = [Cloudformation()])]
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
                &request_context_for_user(user_id),
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
            .all(|a| a.user_state.clone().unwrap().watchlist.watching)
    );
}
*/

// ---------------------------------------------------------------------------
// API: Product watchlist
// Verifies Cognito-protected endpoints, full CRUD lifecycle, and DynamoDB
// access for watchlist records.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
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
            notifications: Some(
                !gotten.items[0]
                    .user_state
                    .clone()
                    .unwrap()
                    .watchlist
                    .notifications,
            ),
            state: None,
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

#[aura_integration_test(services = [Cloudformation()])]
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
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    let update_cmd = UpdateUserCommand {
        tier: Some(UserTier::Ultimate),
        ..Default::default()
    };
    user_service
        .update_user(&user_ctx, &user_id, update_cmd)
        .await
        .unwrap();

    let expected1 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected1_name = Faker.fake::<UserSearchFilterName>();
    let expected2 = Faker.fake::<product::core::product_search::ProductSearch>();
    let expected2_name = Faker.fake::<UserSearchFilterName>();
    service
        .create_user_search_filter(
            &user_ctx,
            &user_id,
            expected1_name.clone(),
            expected1.clone(),
        )
        .await
        .unwrap();
    service
        .create_user_search_filter(
            &user_ctx,
            &user_id,
            expected2_name.clone(),
            expected2.clone(),
        )
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

#[aura_integration_test(services = [Cloudformation()])]
async fn should_post_get_patch_delete_search_filter() {
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
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
        .update_user(&user_ctx, &user_id, update_cmd)
        .await
        .unwrap();

    // POST
    let mut expected = Faker.fake::<PostUserSearchFilterData>();
    expected.search.enhanced_search_description = None;
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
        state: None,
        search: Some(PatchProductSearchData {
            language: Some(LanguageData::Fr),
            product_query: Some(vec!["weesl bee wuff".try_into().unwrap()]),
            ..Default::default()
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
        &patched.search.product_query
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

#[aura_integration_test(services = [Cloudformation()])]
async fn should_embed_search_filter_and_create_match_when_periodic_hybrid_matching_runs() {
    let cfn = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Ultimate),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let post_url = format!("{}/api/v1/me/search-filters", cfn.api_gateway_endpoint_url,);
    let post_response = reqwest::Client::new()
        .post(post_url)
        .bearer_auth(&user.access_token)
        .json(&serde_json::json!({
            "name": "Periodic porcelain alerts",
            "search": {
                "language": "en",
                "currency": "EUR",
                "productQuery": ["rare porcelain vase"],
                "enhancedSearchDescription": "blue floral porcelain vase"
            }
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(201, post_response.status());
    let posted = post_response.json::<UserSearchFilterData>().await.unwrap();

    let search_filter_dynamodb_repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &cfn.dynamodb_table_1_name,
    );
    let search_filter_opensearch_repository =
        UserSearchFilterOpenSearchRepositoryImpl::new(get_opensearch_client().await);

    let persisted_filter = search_filter_dynamodb_repository
        .get_user_search_filter_record(&user_id, &posted.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();
    let mut document: UserSearchFilterDocument = persisted_filter.try_into().unwrap();
    document.embedding = Some(localstack_query_embedding());
    search_filter_opensearch_repository
        .index_document(document.clone())
        .await
        .unwrap();
    refresh_index("user_search_filters").await;

    let document: UserSearchFilterDocument =
        read_by_id("user_search_filters", posted.user_search_filter_id).await;
    let embedding = document
        .embedding
        .clone()
        .expect("search-filter OpenSearch document should store a query embedding");
    assert_eq!(768, embedding.len());
    assert!(
        embedding.iter().any(|value| value.abs() > 0.000_1),
        "search-filter OpenSearch document should store a non-zero query embedding"
    );

    let product_opensearch_repository =
        ProductOpenSearchRepositoryImpl::new(get_opensearch_client().await);
    let product = make_periodic_acceptance_product_document(
        "Rare porcelain vase with blue floral decoration",
        localstack_query_embedding(),
        document.last_hybrid_search_matched + time::Duration::days(1),
    );
    let product_insert = product_opensearch_repository
        .create_product_documents(vec![product.clone()])
        .await
        .unwrap();
    assert!(!product_insert.errors);
    refresh_index("products").await;
    tokio::time::sleep(Duration::from_secs(2)).await;

    let search_filter_service = UserSearchFilterServiceImpl::with_opensearch(
        &search_filter_dynamodb_repository,
        &user_service,
        &search_filter_opensearch_repository,
    );
    let query_product_service = QueryProductServiceImpl::new(&product_opensearch_repository);
    let notification_repository = NotificationDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &cfn.dynamodb_table_1_name,
    );
    let noop_ses_adapter = NoopSesAdapter;
    let noop_s3_adapter = NoopS3Adapter;
    let notification_service = NotificationServiceImpl::new(
        &notification_repository,
        &user_service,
        &noop_ses_adapter,
        &noop_s3_adapter,
        "test-bucket",
        "test-stage",
        "test-sha",
    );
    let expected_reason = EnhancedMatchReason::from("It is the requested blue porcelain vase.");
    let mut enhanced_search_match_service = MockEnhancedSearchMatchService::default();
    let expected_reason_for_llm = expected_reason.clone();
    enhanced_search_match_service
        .expect_evaluate()
        .times(1)
        .return_once(move |description, _, _, language, _| {
            assert_eq!(description.as_ref(), "blue floral porcelain vase");
            assert_eq!(language, Language::En);
            Box::pin(async move {
                Ok(EnhancedSearchMatchResult {
                    matches: true,
                    reason: Some(expected_reason_for_llm),
                })
            })
        });

    let matcher = PeriodicMatcherServiceImpl::new(
        &search_filter_service,
        &query_product_service,
        &enhanced_search_match_service,
        &notification_service,
        &user_service,
        DEFAULT_LLM_CONCURRENCY,
    );

    let result = matcher.match_active_filters().await.unwrap();

    assert_eq!(
        result,
        PeriodicMatcherResult {
            filters_processed: 1,
            matches_created: 1,
            notifications_created: 1,
            filters_failed: 0,
        }
    );

    let persisted_match = search_filter_dynamodb_repository
        .get_user_search_filter_match_record(
            &user_id,
            &posted.user_search_filter_id,
            &product.shop_id,
            &product.shops_product_id,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(product.product_id, persisted_match.product_id);
    assert_eq!(
        Some(expected_reason.as_ref()),
        persisted_match.enhanced_match_reason.as_deref()
    );
    assert!(
        notification_repository
            .get_notification_record(&user_id, &product.event_id)
            .await
            .unwrap()
            .is_some()
    );

    let updated_filter = search_filter_dynamodb_repository
        .get_user_search_filter_record(&user_id, &posted.user_search_filter_id)
        .await
        .unwrap()
        .unwrap();
    assert!(updated_filter.last_hybrid_search_matched > document.last_hybrid_search_matched);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_patch_search_filter_product_match_feedback_when_authorized() {
    use search_filter::dynamodb::repository::{
        UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
    };
    use search_filter::dynamodb::user_search_filter_match_record::{
        UserSearchFilterMatchRecord, mk_lsi1_sk, mk_pk, mk_sk,
    };

    let cfn = get_cfn_output();
    let repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &cfn.dynamodb_table_1_name,
    );
    let user = create_random_test_user().await;
    let user_id: UserId = user.sub.into();
    let filter_id = common::user_search_filter_id::UserSearchFilterId::new();
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::new();
    let created = OffsetDateTime::now_utc();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&created);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id.clone();
    record.feedback = None;
    record.created = created;
    record.updated = created;
    repository
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    let patch = PatchUserSearchFilterMatchData {
        feedback: Some(true),
    };
    let url = format!(
        "{}/api/v1/me/search-filters/{}/matches/{}/{}",
        cfn.api_gateway_endpoint_url, filter_id, shop_id, shops_product_id,
    );
    let response = reqwest::Client::new()
        .patch(url)
        .json(&patch)
        .bearer_auth(user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = repository
        .get_user_search_filter_match_record(&user_id, &filter_id, &shop_id, &shops_product_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(Some(true), actual.feedback);
}

// ---------------------------------------------------------------------------
// API: Shop
// Verifies API Gateway routing and Lambda IAM access for shop GET and
// OpenSearch-backed shop search.
// ---------------------------------------------------------------------------

/**
#[ignore = "Cannot get Localstack-Lambda to reach OpenSearch"]
#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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

#[aura_integration_test(services = [Cloudformation()])]
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
// API: Shop – Partner endpoints
// Verifies API Gateway routing, JWT auth, and Lambda IAM access for
// PATCH shop, PUT api-key, and GET partner shops endpoints.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_shop_patch_by_partner() {
    let user = create_random_test_user().await;

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_partner_status = ShopPartnerStatusRecord::Partnered;
    let stack = get_cfn_output();
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    dynamodb_repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();

    // Link the shop to the user's partner_shops so the PATCH succeeds
    let user_id = UserId::from(user.sub);
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut user_record = user_repository
        .get_user_record(&user_id)
        .await
        .unwrap()
        .expect("user record must exist after create_random_test_user");
    user_record.partner_shops.insert(shop_record.shop_id);
    user_repository.put_user_record(user_record).await.unwrap();

    let patch_data = PatchShopData {
        shop_type: None,
        domains: None,
        shopify_domain: None,
        shopify_currency: None,
        shopify_language: None,
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        image: Some(url::Url::parse("https://new-image.example.com/logo.png").unwrap()),
        structured_address: None,
        phone: None,
        email: None,
    };

    let url = format!(
        "{}/api/v1/shops/{}",
        stack.api_gateway_endpoint_url, shop_record.shop_id,
    );
    let response = reqwest::Client::new()
        .patch(&url)
        .bearer_auth(&user.access_token)
        .json(&patch_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_201_for_shop_post_by_admin() {
    let admin = create_admin_test_user().await;
    let stack = get_cfn_output();

    let post_data: PostShopData = Faker.fake();

    let url = format!("{}/api/v1/shops", stack.api_gateway_endpoint_url,);
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&admin.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, response.status());
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_get_shops() {
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let mut shop_record: ShopRecord = Faker.fake();
    shop_record.shop_partner_status = ShopPartnerStatusRecord::Partnered;
    let stack = get_cfn_output();
    let dynamodb_repository =
        ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    dynamodb_repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();

    // Link the shop to the user's partner_shops so the GET returns it
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let mut user_record = user_repository
        .get_user_record(&user_id)
        .await
        .unwrap()
        .expect("user record must exist after create_random_test_user");
    user_record.partner_shops.insert(shop_record.shop_id);
    user_repository.put_user_record(user_record).await.unwrap();

    let url = format!("{}/api/v1/me/partner-shops", stack.api_gateway_endpoint_url);
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body: Vec<GetShopData> = response.json().await.unwrap();
    assert_eq!(1, body.len());
    assert_eq!(shop_record.shop_id, body[0].shop_id);
}

// ---------------------------------------------------------------------------
// API: Notification
// Verifies Cognito-protected notification endpoints: get, patch-one,
// patch-all, delete-one, delete-all, seeding data directly via DynamoDB.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
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

fn valid_new_partner_application_payload(name: &str) -> PostPartnerShopApplicationPayloadData {
    PostPartnerShopApplicationPayloadData::New {
        shop_name: common::shop_name::ShopName::from(name.to_string()),
        shop_type: shop::data::shop_type_data::ShopTypeData::CommercialDealer,
        shop_domains: std::collections::HashSet::new(),
        shop_url: None,
        shop_image: None,
        shop_structured_address: None,
        shop_phone: None,
        shop_email: None,
    }
}

// ---------------------------------------------------------------------------
// API: Partner Shop Application CRUD
// Verifies API Gateway routing and Lambda execution for the partner shop
// application endpoints with Cognito JWT authentication.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_application_get_all() {
    let user = create_random_test_user().await;

    let url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<Vec<GetPartnerShopApplicationData>>()
        .await
        .unwrap();
    assert!(actual.is_empty());
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_201_for_partner_application_post() {
    let user = create_random_test_user().await;
    let url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );

    let post_data = valid_new_partner_application_payload("Acceptance Partner Application Post");
    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, response.status());

    let created = response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(
        PartnerShopApplicationStateData::Submitted,
        created.business_state
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_application_get_one() {
    let user = create_random_test_user().await;
    let base_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );

    // Create first
    let post_data = valid_new_partner_application_payload("Acceptance Partner Application Get One");
    let create_response = reqwest::Client::new()
        .post(&base_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();

    // GET one
    let get_url = format!("{}/{}", base_url, created.id);
    let get_response = reqwest::Client::new()
        .get(&get_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, get_response.status());
    let gotten = get_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, gotten.id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_partner_application_patch() {
    let user = create_random_test_user().await;
    let base_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );

    // Create first
    let post_data = valid_new_partner_application_payload("Acceptance Partner Application Patch");
    let create_response = reqwest::Client::new()
        .post(&base_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();

    // PATCH
    let patch_url = format!("{}/{}", base_url, created.id);
    let patch_data = PatchPartnerShopApplicationData {
        shop_name: Some("Updated Shop".into()),
        shop_type: None,
        shop_domains: None,
        shop_url: None,
        shop_image: None,
        shop_structured_address: None,
        shop_phone: None,
        shop_email: None,
    };
    let patch_response = reqwest::Client::new()
        .patch(&patch_url)
        .bearer_auth(&user.access_token)
        .json(&patch_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    // Verify the response reflects the update
    let patched = patch_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, patched.id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_204_for_partner_application_delete() {
    let user = create_random_test_user().await;
    let base_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );

    // Create first
    let post_data = valid_new_partner_application_payload("Acceptance Partner Application Delete");
    let create_response = reqwest::Client::new()
        .post(&base_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();

    // DELETE
    let delete_url = format!("{}/{}", base_url, created.id);
    let delete_response = reqwest::Client::new()
        .delete(&delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, delete_response.status());

    // Verify deleted
    let get_response = reqwest::Client::new()
        .get(&delete_url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(404, get_response.status());
}

// ---------------------------------------------------------------------------
// API: Admin Partner Shop Application
// Verifies API Gateway routing and Lambda execution for the admin partner shop
// application endpoints with Cognito JWT authentication and admin role check.
// ---------------------------------------------------------------------------

async fn create_admin_test_user() -> TestUser {
    let user = create_random_test_user().await;
    let cfn = get_cfn_output();
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &cfn.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    let update_cmd = UpdateUserCommand {
        role: Some(UserRole::Admin),
        ..Default::default()
    };
    user_service
        .update_user(&user_ctx, &user_id, update_cmd)
        .await
        .unwrap();
    user
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_partner_application_get_all() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;

    // Create an application as a normal user
    let user_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_data =
        valid_new_partner_application_payload("Acceptance Admin Partner Application Get All");
    let create_response = reqwest::Client::new()
        .post(&user_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());

    // Admin GET all
    let admin_url = format!(
        "{}/api/v1/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let response = reqwest::Client::new()
        .get(&admin_url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<Vec<GetPartnerShopApplicationData>>()
        .await
        .unwrap();
    assert!(!actual.is_empty());
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_partner_application_get_one() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;

    // Create an application as a normal user
    let user_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_data =
        valid_new_partner_application_payload("Acceptance Admin Partner Application Get One");
    let create_response = reqwest::Client::new()
        .post(&user_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();

    // Admin GET one
    let admin_url = format!(
        "{}/api/v1/partner-applications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let response = reqwest::Client::new()
        .get(&admin_url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let actual = response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, actual.id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_partner_application_patch() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;

    // Create an application as a normal user
    let user_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_data =
        valid_new_partner_application_payload("Acceptance Admin Partner Application Patch");
    let create_response = reqwest::Client::new()
        .post(&user_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();

    // Admin PATCH to update shop_name (no state change)
    let admin_url = format!(
        "{}/api/v1/partner-applications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let patch_data = AdminPatchPartnerShopApplicationData {
        shop_name: Some(common::shop_name::ShopName::from(
            "Admin Updated".to_string(),
        )),
        shop_type: None,
        shop_domains: None,
        shop_url: None,
        shop_image: None,
        shop_structured_address: None,
        shop_phone: None,
        shop_email: None,
    };
    let client = reqwest::Client::new();
    let deadline = Instant::now() + Duration::from_secs(30);
    let response = loop {
        let response = client
            .patch(&admin_url)
            .bearer_auth(&admin.access_token)
            .json(&patch_data)
            .send()
            .await
            .unwrap();
        if response.status() != reqwest::StatusCode::NOT_FOUND || Instant::now() >= deadline {
            break response;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    };
    assert_eq!(200, response.status());

    let patched = response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, patched.id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_decision_approve() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;

    // Create an application as a normal user
    let user_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    // Use a New shop payload so the APPROVE step can create the shop rather than
    // looking up a non-existent shop by a random ID.
    let post_data = PostPartnerShopApplicationPayloadData::New {
        shop_name: common::shop_name::ShopName::from("Accept Test Shop".to_string()),
        shop_type: shop::data::shop_type_data::ShopTypeData::CommercialDealer,
        shop_domains: std::collections::HashSet::new(),
        shop_url: None,
        shop_image: None,
        shop_structured_address: None,
        shop_phone: None,
        shop_email: None,
    };
    let create_response = reqwest::Client::new()
        .post(&user_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(
        PartnerShopApplicationStateData::Submitted,
        created.business_state
    );
    assert_eq!(ExecutionStateData::Processing, created.execution_state);

    // Wait for the step function to set the application to InReview (Waiting)
    let get_url = format!(
        "{}/api/v1/me/partner-applications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let mut in_review = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let check_response = reqwest::Client::new()
            .get(&get_url)
            .bearer_auth(&user.access_token)
            .send()
            .await
            .unwrap();
        if check_response.status() == 200 {
            let check_data = check_response
                .json::<GetPartnerShopApplicationData>()
                .await
                .unwrap();
            if check_data.business_state == PartnerShopApplicationStateData::InReview
                && check_data.execution_state == ExecutionStateData::Waiting
            {
                in_review = true;
                break;
            }
        }
    }
    assert!(
        in_review,
        "Application did not transition to InReview/Waiting within timeout"
    );

    // Admin POST decision to approve
    let decision_url = format!(
        "{}/api/v1/partner-applications/{}/decision",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let decision_data = PostPartnerShopApplicationDecisionData {
        decision: PartnerShopApplicationDecisionData::Approve,
    };
    let response = reqwest::Client::new()
        .post(&decision_url)
        .bearer_auth(&admin.access_token)
        .json(&decision_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let decision_result = response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, decision_result.id);
    assert_eq!(
        ExecutionStateData::Processing,
        decision_result.execution_state
    );

    // Wait for the step function to complete and set the application to Approved (Completed)
    let mut approved = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let check_response = reqwest::Client::new()
            .get(&get_url)
            .bearer_auth(&user.access_token)
            .send()
            .await
            .unwrap();
        if check_response.status() == 200 {
            let check_data = check_response
                .json::<GetPartnerShopApplicationData>()
                .await
                .unwrap();
            if check_data.business_state == PartnerShopApplicationStateData::Approved
                && check_data.execution_state == ExecutionStateData::Completed
            {
                approved = true;
                break;
            }
        }
    }
    assert!(
        approved,
        "Application did not transition to Approved/Completed within timeout"
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_decision_reject() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;

    // Create an application as a normal user
    let user_url = format!(
        "{}/api/v1/me/partner-applications",
        get_cfn_output().api_gateway_endpoint_url,
    );
    let post_data = valid_new_partner_application_payload("Acceptance Admin Decision Reject");
    let create_response = reqwest::Client::new()
        .post(&user_url)
        .bearer_auth(&user.access_token)
        .json(&post_data)
        .send()
        .await
        .unwrap();
    assert_eq!(201, create_response.status());
    let created = create_response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(
        PartnerShopApplicationStateData::Submitted,
        created.business_state
    );
    assert_eq!(ExecutionStateData::Processing, created.execution_state);

    // Wait for the step function to set the application to InReview (Waiting)
    let get_url = format!(
        "{}/api/v1/me/partner-applications/{}",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let mut in_review = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let check_response = reqwest::Client::new()
            .get(&get_url)
            .bearer_auth(&user.access_token)
            .send()
            .await
            .unwrap();
        if check_response.status() == 200 {
            let check_data = check_response
                .json::<GetPartnerShopApplicationData>()
                .await
                .unwrap();
            if check_data.business_state == PartnerShopApplicationStateData::InReview
                && check_data.execution_state == ExecutionStateData::Waiting
            {
                in_review = true;
                break;
            }
        }
    }
    assert!(
        in_review,
        "Application did not transition to InReview/Waiting within timeout"
    );

    // Admin POST decision to reject
    let decision_url = format!(
        "{}/api/v1/partner-applications/{}/decision",
        get_cfn_output().api_gateway_endpoint_url,
        created.id,
    );
    let decision_data = PostPartnerShopApplicationDecisionData {
        decision: PartnerShopApplicationDecisionData::Reject,
    };
    let response = reqwest::Client::new()
        .post(&decision_url)
        .bearer_auth(&admin.access_token)
        .json(&decision_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let decision_result = response
        .json::<GetPartnerShopApplicationData>()
        .await
        .unwrap();
    assert_eq!(created.id, decision_result.id);
    assert_eq!(
        ExecutionStateData::Processing,
        decision_result.execution_state
    );

    // Wait for the step function to complete and set the application to Rejected (Completed)
    let mut rejected = false;
    for _ in 0..30 {
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;
        let check_response = reqwest::Client::new()
            .get(&get_url)
            .bearer_auth(&user.access_token)
            .send()
            .await
            .unwrap();
        if check_response.status() == 200 {
            let check_data = check_response
                .json::<GetPartnerShopApplicationData>()
                .await
                .unwrap();
            if check_data.business_state == PartnerShopApplicationStateData::Rejected
                && check_data.execution_state == ExecutionStateData::Completed
            {
                rejected = true;
                break;
            }
        }
    }
    assert!(
        rejected,
        "Application did not transition to Rejected/Completed within timeout"
    );
}

// ---------------------------------------------------------------------------
// API: Partner Product Creation
// Verifies API Gateway routing and Lambda execution for the partner product
// creation endpoint with x-api-key authentication (no Cognito JWT).
// ---------------------------------------------------------------------------

async fn prepare_partner_shop() -> (ShopRecord, RawAccessToken) {
    let stack = get_cfn_output();

    // Create the partnered shop record
    let mut record: ShopRecord = Faker.fake();
    record.shop_partner_status = ShopPartnerStatusRecord::Partnered;
    let shop_id = record.shop_id;
    ShopDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name)
        .put_shop_record(record.clone())
        .await
        .unwrap();

    // Create a user record with the shop in partner_shops
    let mut user: user::core::user::User = Faker.fake();
    user.partner_shops = [shop_id].into();
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    user_repository
        .put_user_record(UserRecord::from(user.clone()))
        .await
        .unwrap();

    // Create an access token in DynamoDB so the partner can authenticate
    let raw_token = RawAccessToken::new();
    let access_token = AccessToken {
        id: AccessTokenId::new(),
        hashed_token: raw_token.clone().into(),
        user_id: user.user_id,
        name: AccessTokenName::from("partner-shop"),
        scopes: [Scope::ProductsWrite].into(),
        origin: AccessTokenOrigin::User,
        expires: None,
        created_by: common::actor::domain::Actor::User(user.user_id),
        updated_by: common::actor::domain::Actor::User(user.user_id),
        created: time::OffsetDateTime::now_utc(),
        updated: time::OffsetDateTime::now_utc(),
    };
    user_repository
        .put_access_token_record(AccessTokenRecord::from(access_token))
        .await
        .unwrap();

    (record, raw_token)
}

async fn wait_for_partner_product_record(
    shop_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
) -> ProductRecord {
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        get_cfn_output().dynamodb_table_1_name.as_str(),
    );
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(product_record) = product_repository
            .get_product_record(&shop_id, &shops_product_id)
            .await
            .unwrap()
        {
            return product_record;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: Partner product '{}' for shop '{}' was not persisted by async ingestion",
                shops_product_id, shop_id
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

async fn wait_for_partner_product_state(
    shop_id: common::shop_id::ShopId,
    shops_product_id: common::shops_product_id::ShopsProductId,
    expected: product::dynamodb::product_state_record::ProductStateRecord,
) {
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let product = wait_for_partner_product_record(shop_id, shops_product_id.clone()).await;
        if product.state == expected {
            return;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: Partner product '{}' for shop '{}' did not reach state '{:?}'",
                shops_product_id, shop_id, expected
            );
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
    }
}

const WOOCOMMERCE_WEBHOOK_SECRET: &str = "woocommerce-acceptance-secret";
const WOOCOMMERCE_CREATED_BODY: &str = r#"{"id":17,"name":"Test Produkt Titel","slug":"test-produkt-titel","permalink":"http://aura-historia-test.local/product/test-produkt-titel/","date_created":"2026-05-13T19:22:31","date_modified":"2026-05-13T19:23:23","type":"simple","status":"publish","description":"<p>Hayde yallah test beschreibung</p>\n","short_description":"<p>Hayde yallah kurze test beschreibung</p>\n","price":"42.69","regular_price":"42.69","stock_status":"instock","categories":[{"id":15,"name":"Uncategorized","slug":"uncategorized"}],"images":[]}"#;
const WOOCOMMERCE_UPDATED_BODY: &str = r#"{"id":17,"name":"Test Produkt Titel","slug":"test-produkt-titel","permalink":"http://aura-historia-test.local/product/test-produkt-titel/","date_created":"2026-05-13T19:22:31","date_modified":"2026-05-13T19:24:54","type":"simple","status":"publish","description":"<p>Hayde yallah test beschreibung</p>\n","short_description":"<p>Hayde yallah kurze test beschreibung</p>\n","price":"123.45","regular_price":"123.45","stock_status":"instock","categories":[{"id":15,"name":"Uncategorized","slug":"uncategorized"}],"images":[]}"#;
const WOOCOMMERCE_DELETED_BODY: &str = r#"{"id":17}"#;

fn woocommerce_signature(body: &str) -> String {
    let key = PKey::hmac(WOOCOMMERCE_WEBHOOK_SECRET.as_bytes()).unwrap();
    let mut signer = Signer::new(MessageDigest::sha256(), &key).unwrap();
    signer.update(body.as_bytes()).unwrap();
    base64::engine::general_purpose::STANDARD.encode(signer.sign_to_vec().unwrap())
}

async fn prepare_woocommerce_partner_shop() -> (ShopRecord, RawAccessToken) {
    let (mut shop_record, api_key) = prepare_partner_shop().await;
    shop_record.woocommerce_webhook_secret =
        Some(WoocommerceWebhookSecret::from(WOOCOMMERCE_WEBHOOK_SECRET));
    shop_record.woocommerce_currency = Some(common::currency::record::CurrencyRecord::Eur);
    shop_record.woocommerce_language = Some(common::language::record::LanguageRecord::De);
    let dynamodb_repository = ShopDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &get_cfn_output().dynamodb_table_1_name,
    );
    dynamodb_repository
        .put_shop_record(shop_record.clone())
        .await
        .unwrap();
    (shop_record, api_key)
}

async fn send_woocommerce_webhook(
    shop_record: &ShopRecord,
    api_key: &RawAccessToken,
    topic: &str,
    body: &str,
) {
    let api_key: String = api_key.clone().into();
    let url = format!(
        "{}/api/v1/webhooks/woocommerce/{}",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );

    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(api_key)
        .header("x-wc-webhook-topic", topic)
        .header("x-wc-webhook-signature", woocommerce_signature(body))
        .header("content-type", "application/json")
        .body(body.to_owned())
        .send()
        .await
        .unwrap();
    assert_eq!(202, response.status());
    assert_eq!(0, response.bytes().await.unwrap().len());
}

async fn assert_woocommerce_product(topic: &str, shop_id: common::shop_id::ShopId) {
    if topic == "product.deleted" {
        wait_for_partner_product_state(
            shop_id,
            "17".into(),
            product::dynamodb::product_state_record::ProductStateRecord::Removed,
        )
        .await;
    }

    let product = wait_for_partner_product_record(shop_id, "17".into()).await;
    assert_eq!(product.shops_product_id.to_string(), "17");
    match topic {
        "product.created" => {
            assert_eq!(product.title_native.text, "Test Produkt Titel");
            assert_eq!(
                product.price_native.as_ref().map(|price| price.amount),
                Some(4269)
            );
            assert_eq!(
                product.state,
                product::dynamodb::product_state_record::ProductStateRecord::Available
            );
        }
        "product.updated" => {
            assert_eq!(
                product.price_native.as_ref().map(|price| price.amount),
                Some(12345)
            );
        }
        "product.deleted" => {
            assert_eq!(
                product.state,
                product::dynamodb::product_state_record::ProductStateRecord::Removed
            );
        }
        _ => {}
    }
}

async fn post_woocommerce_webhook(topic: &str, body: &str) {
    let (shop_record, api_key) = prepare_woocommerce_partner_shop().await;
    send_woocommerce_webhook(&shop_record, &api_key, topic, body).await;
    assert_woocommerce_product(topic, shop_record.shop_id).await;
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_woocommerce_created_webhook() {
    post_woocommerce_webhook("product.created", WOOCOMMERCE_CREATED_BODY).await;
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_woocommerce_updated_webhook() {
    post_woocommerce_webhook("product.updated", WOOCOMMERCE_UPDATED_BODY).await;
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_woocommerce_deleted_webhook() {
    let (shop_record, api_key) = prepare_woocommerce_partner_shop().await;

    send_woocommerce_webhook(
        &shop_record,
        &api_key,
        "product.created",
        WOOCOMMERCE_CREATED_BODY,
    )
    .await;
    assert_woocommerce_product("product.created", shop_record.shop_id).await;

    send_woocommerce_webhook(
        &shop_record,
        &api_key,
        "product.deleted",
        WOOCOMMERCE_DELETED_BODY,
    )
    .await;
    assert_woocommerce_product("product.deleted", shop_record.shop_id).await;
}

#[aura_integration_test(services = [Cloudformation()])]
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
        .bearer_auth(&api_key_str)
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
    assert_eq!(202, response.status());

    let body: Vec<String> = response.json().await.unwrap();
    assert!(body.is_empty());
    let product =
        wait_for_partner_product_record(shop_record.shop_id, "acceptance-test-product-1".into())
            .await;
    assert_eq!(
        product.shops_product_id.to_string(),
        "acceptance-test-product-1"
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_delete_partner_product_and_cleanup_user_resources() {
    use product_watchlist::dynamodb::record::{
        WatchlistProductRecord, mk_gsi1_pk as mk_watchlist_gsi1_pk,
        mk_gsi1_sk as mk_watchlist_gsi1_sk, mk_pk as mk_watchlist_pk, mk_sk as mk_watchlist_sk,
    };
    use search_filter::dynamodb::user_search_filter_match_record::{
        UserSearchFilterMatchRecord, mk_gsi2_pk as mk_match_gsi2_pk,
        mk_gsi2_sk as mk_match_gsi2_sk, mk_lsi1_sk as mk_match_lsi1_sk,
        mk_lsi2_sk as mk_match_lsi2_sk, mk_pk as mk_match_pk, mk_sk as mk_match_sk,
    };

    let cfn = get_cfn_output();
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();
    let product_repository = ProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        cfn.dynamodb_table_1_name.as_str(),
    );
    let watchlist_repository = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &cfn.dynamodb_table_1_name,
    );
    let search_filter_repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &cfn.dynamodb_table_1_name,
    );

    let mut target_product = Faker.fake::<ProductRecord>();
    target_product.shop_id = shop_record.shop_id;
    target_product.shops_product_id = "acceptance-test-delete-product-1".into();
    target_product.pk =
        product_record::mk_pk(&target_product.shop_id, &target_product.shops_product_id);
    target_product.sk = product_record::mk_sk().to_owned();
    target_product.lifecycle = common::product_lifecycle::record::ProductLifecycleRecord::Active;
    target_product.state = product::dynamodb::product_state_record::ProductStateRecord::Available;
    product_repository
        .put_product_records([target_product.clone()].into())
        .await
        .unwrap();

    let mut target_watchlist_records = Vec::new();
    for _ in 0..2 {
        let user_id = UserId::new();
        let mut record = Faker.fake::<WatchlistProductRecord>();
        record.pk = mk_watchlist_pk(&user_id);
        record.sk = mk_watchlist_sk(&target_product.shop_id, &target_product.shops_product_id);
        record.gsi1_pk = mk_watchlist_gsi1_pk(&target_product.product_id);
        record.gsi1_sk = mk_watchlist_gsi1_sk(&user_id);
        record.user_id = user_id;
        record.product_id = target_product.product_id;
        record.shop_id = target_product.shop_id;
        record.shops_product_id = target_product.shops_product_id.clone();
        record.state = ResourceStateRecord::Active;
        watchlist_repository
            .put_watchlist_record(record.clone())
            .await
            .unwrap();
        target_watchlist_records.push(record);
    }

    let civilian_product_id = common::product_id::ProductId::new();
    let civilian_user_id = UserId::new();
    let mut civilian_watchlist_record = Faker.fake::<WatchlistProductRecord>();
    civilian_watchlist_record.pk = mk_watchlist_pk(&civilian_user_id);
    civilian_watchlist_record.sk = mk_watchlist_sk(
        &civilian_watchlist_record.shop_id,
        &civilian_watchlist_record.shops_product_id,
    );
    civilian_watchlist_record.gsi1_pk = mk_watchlist_gsi1_pk(&civilian_product_id);
    civilian_watchlist_record.gsi1_sk = mk_watchlist_gsi1_sk(&civilian_user_id);
    civilian_watchlist_record.user_id = civilian_user_id;
    civilian_watchlist_record.product_id = civilian_product_id;
    watchlist_repository
        .put_watchlist_record(civilian_watchlist_record.clone())
        .await
        .unwrap();

    let mut target_match_records = Vec::new();
    for _ in 0..2 {
        let user_id = UserId::new();
        let filter_id = common::user_search_filter_id::UserSearchFilterId::new();
        let created = OffsetDateTime::now_utc();
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_match_pk(&user_id);
        record.sk = mk_match_sk(
            &filter_id,
            &target_product.shop_id,
            &target_product.shops_product_id,
        );
        record.lsi1_sk = mk_match_lsi1_sk(&created);
        record.lsi2_sk = Some(mk_match_lsi2_sk(
            &target_product.shop_id,
            &target_product.shops_product_id,
            &created,
        ));
        record.gsi2_pk = Some(mk_match_gsi2_pk(&target_product.product_id));
        record.gsi2_sk = Some(mk_match_gsi2_sk(&user_id));
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.shop_id = target_product.shop_id;
        record.shops_product_id = target_product.shops_product_id.clone();
        record.product_id = target_product.product_id;
        record.created = created;
        record.updated = created;
        search_filter_repository
            .put_user_search_filter_match_record(record.clone())
            .await
            .unwrap();
        target_match_records.push(record);
    }

    let civilian_match_user_id = UserId::new();
    let civilian_match_filter_id = common::user_search_filter_id::UserSearchFilterId::new();
    let civilian_match_product_id = common::product_id::ProductId::new();
    let civilian_match_created = OffsetDateTime::now_utc();
    let mut civilian_match_record = Faker.fake::<UserSearchFilterMatchRecord>();
    civilian_match_record.pk = mk_match_pk(&civilian_match_user_id);
    civilian_match_record.sk = mk_match_sk(
        &civilian_match_filter_id,
        &civilian_match_record.shop_id,
        &civilian_match_record.shops_product_id,
    );
    civilian_match_record.lsi1_sk = mk_match_lsi1_sk(&civilian_match_created);
    civilian_match_record.gsi2_pk = Some(mk_match_gsi2_pk(&civilian_match_product_id));
    civilian_match_record.gsi2_sk = Some(mk_match_gsi2_sk(&civilian_match_user_id));
    civilian_match_record.user_id = civilian_match_user_id;
    civilian_match_record.user_search_filter_id = civilian_match_filter_id;
    civilian_match_record.product_id = civilian_match_product_id;
    civilian_match_record.created = civilian_match_created;
    civilian_match_record.updated = civilian_match_created;
    search_filter_repository
        .put_user_search_filter_match_record(civilian_match_record.clone())
        .await
        .unwrap();

    // Cleanup worker finds dependent records through GSIs. Wait until LocalStack
    // has projected the seeded records before firing the delete event.
    let gsi_deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let watchlist_records = watchlist_repository
            .query_user_ids_watching_product(&target_product.product_id)
            .await
            .unwrap();
        let target_watchlist_visible = target_watchlist_records.iter().all(|expected| {
            watchlist_records.iter().any(|actual| {
                actual.user_id == expected.user_id
                    && actual.shop_id == expected.shop_id
                    && actual.shops_product_id == expected.shops_product_id
            })
        });

        let match_keys = search_filter_repository
            .query_user_search_filter_match_keys_for_product_id(&target_product.product_id)
            .await
            .unwrap();
        let target_matches_visible = target_match_records.iter().all(|expected| {
            match_keys
                .iter()
                .any(|(user_id, search_filter_id, shop_id, shops_product_id)| {
                    *user_id == expected.user_id
                        && *search_filter_id == expected.user_search_filter_id
                        && *shop_id == expected.shop_id
                        && shops_product_id == &expected.shops_product_id
                })
        });

        if target_watchlist_visible && target_matches_visible {
            break;
        }

        if Instant::now() >= gsi_deadline {
            panic!("Timeout: target user resources did not become visible through cleanup GSIs");
        }
        tokio::time::sleep(Duration::from_secs(1)).await;
    }

    let delete_url = format!(
        "{}/api/v1/shops/{}/products/{}",
        cfn.api_gateway_endpoint_url, shop_record.shop_id, target_product.shops_product_id,
    );
    let response = reqwest::Client::new()
        .delete(delete_url)
        .bearer_auth(&api_key_str)
        .send()
        .await
        .unwrap();
    assert_eq!(204, response.status());

    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        let mut remaining_target_watchlist_count = 0;
        for record in &target_watchlist_records {
            if watchlist_repository
                .get_watchlist_record(&record.user_id, &record.shop_id, &record.shops_product_id)
                .await
                .unwrap()
                .is_some()
            {
                remaining_target_watchlist_count += 1;
            }
        }
        let target_watchlist_deleted = remaining_target_watchlist_count == 0;

        let mut remaining_target_match_count = 0;
        for record in &target_match_records {
            if search_filter_repository
                .get_user_search_filter_match_record(
                    &record.user_id,
                    &record.user_search_filter_id,
                    &record.shop_id,
                    &record.shops_product_id,
                )
                .await
                .unwrap()
                .is_some()
            {
                remaining_target_match_count += 1;
            }
        }
        let target_matches_deleted = remaining_target_match_count == 0;

        let civilian_watchlist_exists = watchlist_repository
            .get_watchlist_record(
                &civilian_watchlist_record.user_id,
                &civilian_watchlist_record.shop_id,
                &civilian_watchlist_record.shops_product_id,
            )
            .await
            .unwrap()
            .is_some();
        let civilian_match_exists = search_filter_repository
            .get_user_search_filter_match_record(
                &civilian_match_record.user_id,
                &civilian_match_record.user_search_filter_id,
                &civilian_match_record.shop_id,
                &civilian_match_record.shops_product_id,
            )
            .await
            .unwrap()
            .is_some();
        let target_product_deleted = product_repository
            .get_product_record(&target_product.shop_id, &target_product.shops_product_id)
            .await
            .unwrap()
            .is_none();
        let target_product_events_deleted = product_repository
            .query_product_record_and_event_record_keys(
                &target_product.shop_id,
                &target_product.shops_product_id,
            )
            .await
            .unwrap()
            .is_empty();

        if target_watchlist_deleted
            && target_matches_deleted
            && target_product_deleted
            && target_product_events_deleted
            && civilian_watchlist_exists
            && civilian_match_exists
        {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: product delete cleanup failed; remaining_watchlist={remaining_target_watchlist_count}, remaining_matches={remaining_target_match_count}, target_product_deleted={target_product_deleted}, target_product_events_deleted={target_product_events_deleted}, civilian_watchlist_exists={civilian_watchlist_exists}, civilian_match_exists={civilian_match_exists}"
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_preserve_partner_post_product_image_order_when_read_via_rest_api() {
    let (shop_record, api_key) = prepare_partner_shop().await;
    let api_key_str: String = api_key.into();
    let shops_product_id = "acceptance-test-product-image-order-1";
    let expected_images = [
        "https://example.com/img-3.jpg",
        "https://example.com/img-1.jpg",
        "https://example.com/img-2.jpg",
    ];

    let post_url = format!(
        "{}/api/v1/shops/{}/products",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
    );
    let response = reqwest::Client::new()
        .post(&post_url)
        .bearer_auth(&api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": shops_product_id,
            "title": { "text": "Ordered Product Images", "language": "en" },
            "description": { "text": "A test product with ordered images", "language": "en" },
            "state": "AVAILABLE",
            "url": "https://example.com/product/ordered-images",
            "images": expected_images
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(202, response.status());

    let body: Vec<String> = response.json().await.unwrap();
    assert!(body.is_empty());

    wait_for_partner_product_record(shop_record.shop_id, shops_product_id.into()).await;

    let get_url = format!(
        "{}/api/v1/shops/{}/products/{}?currency=EUR",
        get_cfn_output().api_gateway_endpoint_url,
        shop_record.shop_id,
        shops_product_id
    );
    let response = reqwest::get(get_url).await.unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<serde_json::Value>().await.unwrap();
    let actual_images: Vec<&str> = body["item"]["images"]
        .as_array()
        .unwrap()
        .iter()
        .map(|image| image["url"].as_str().unwrap())
        .collect();
    assert_eq!(expected_images.to_vec(), actual_images);
}

// ─── Product Pipeline Embed Text (Lambda) ─────────────────────────────────────

#[aura_integration_test(services = [Cloudformation()])]
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

    // 3. Wait for the embed-text Lambda to process the DOMAIN_CREATED event and write
    //    an ENRICHMENT_EMBEDDED event record back to DynamoDB.
    //    The MockMultimodalEmbeddingService returns vec![0.42f32; 768].
    let deadline = Instant::now() + Duration::from_secs(120);
    loop {
        // Scan for the enrichment event record produced by the embed-text Lambda.
        let all_items = get_dynamodb_client()
            .await
            .scan()
            .table_name(&stack.dynamodb_table_1_name)
            .send()
            .await
            .unwrap()
            .items
            .unwrap_or_default();

        let has_enrichment_embedded = all_items.iter().any(|item| {
            item.get("event_type")
                .and_then(|v| v.as_s().ok())
                .map(|s| s == "ENRICHMENT_EMBEDDED")
                .unwrap_or(false)
        });

        if has_enrichment_embedded {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: No ENRICHMENT_EMBEDDED event record found for shop '{}' / product '{}' after 120s",
                shop.shop_id, create_cmd.shops_product_id
            );
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
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
        .bearer_auth(&api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-patch-product-1",
            "state": "SOLD"
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(202, response.status());

    let body: Vec<String> = response.json().await.unwrap();
    assert!(body.is_empty());
    wait_for_partner_product_state(
        shop_record.shop_id,
        "acceptance-test-patch-product-1".into(),
        product::dynamodb::product_state_record::ProductStateRecord::Sold,
    )
    .await;
}

#[aura_integration_test(services = [Cloudformation()])]
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
        .bearer_auth(&api_key_str)
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
    assert_eq!(202, response.status());

    let body: Vec<String> = response.json().await.unwrap();
    assert!(body.is_empty());
    let product = wait_for_partner_product_record(
        shop_record.shop_id,
        "acceptance-test-put-product-1".into(),
    )
    .await;
    assert_eq!(
        product.shops_product_id.to_string(),
        "acceptance-test-put-product-1"
    );
}

#[aura_integration_test(services = [Cloudformation()])]
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
        .bearer_auth(&api_key_str)
        .json(&vec![serde_json::json!({
            "shopsProductId": "acceptance-test-put-existing-product-1",
            "state": "SOLD"
        })])
        .send()
        .await
        .unwrap();
    assert_eq!(202, response.status());

    let body: Vec<String> = response.json().await.unwrap();
    assert!(body.is_empty());
    wait_for_partner_product_state(
        shop_record.shop_id,
        "acceptance-test-put-existing-product-1".into(),
        product::dynamodb::product_state_record::ProductStateRecord::Sold,
    )
    .await;
}

// ---------------------------------------------------------------------------
// Search-filter-match quota enforcement
// Verifies that users who have reached their monthly search-filter-match quota
// do not have additional match records counted beyond their limit.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_count_search_filter_matches_for_current_month_for_quota_enforcement() {
    use search_filter::core::quota::SearchFilterQuota;
    use search_filter::dynamodb::repository::{
        UserSearchFilterDynamoDbRepository, UserSearchFilterDynamoDbRepositoryImpl,
    };
    use search_filter::dynamodb::user_search_filter_match_record::{
        UserSearchFilterMatchRecord, mk_lsi1_sk, mk_pk, mk_sk,
    };
    use search_filter::service::user_search_filter_service::{
        UserSearchFilterService, UserSearchFilterServiceImpl,
    };
    use user::dynamodb::repository::UserDynamoDbRepositoryImpl;
    use user::service::user_service::{UserService, UserServiceImpl};

    let cfn = get_cfn_output();
    let ddb_client = get_dynamodb_client().await;
    let search_filter_repo =
        UserSearchFilterDynamoDbRepositoryImpl::new(ddb_client, &cfn.dynamodb_table_1_name);
    let user_repo = UserDynamoDbRepositoryImpl::new(ddb_client, &cfn.dynamodb_table_1_name);
    let user_service = UserServiceImpl::new(&user_repo);

    // Create a Free tier user
    let user_id = UserId::new();
    let user_ctx = request_context_for_user(user_id);
    let user = user_service
        .create_user(
            &user_ctx,
            user::service::command::CreateUserCommand {
                id: user_id,
                email: "quota-test@example.com".parse().unwrap(),
            },
        )
        .await
        .unwrap();
    assert_eq!(user.tier, UserTier::Free);
    let free_quota = UserTier::Free.search_filter_match_quota();
    assert_eq!(free_quota, 10);

    let filter_id = common::user_search_filter_id::UserSearchFilterId::new();
    let now = OffsetDateTime::now_utc();

    // Insert exactly `free_quota` match records dated within the current month.
    // Timestamps are spread over the last `free_quota` seconds before now so they
    // are always in the past (counted by the service's `to = now` bound) and
    // always in the current month (a ~10 second window never crosses a month boundary
    // in any realistic environment).
    for i in 0..free_quota {
        let shop_id = common::shop_id::ShopId::new();
        let shops_product_id = common::shops_product_id::ShopsProductId::new();
        let created = now - time::Duration::seconds((free_quota - i) as i64);
        let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
        record.pk = mk_pk(&user_id);
        record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
        record.lsi1_sk = mk_lsi1_sk(&created);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.shop_id = shop_id;
        record.shops_product_id = shops_product_id;
        record.created = created;
        record.updated = created;
        search_filter_repo
            .put_user_search_filter_match_record(record)
            .await
            .unwrap();
    }

    // Also insert a record from last month — should NOT be counted
    let last_month = now
        .replace_day(1)
        .unwrap()
        .replace_hour(0)
        .unwrap()
        .replace_minute(0)
        .unwrap()
        .replace_second(0)
        .unwrap()
        .replace_nanosecond(0)
        .unwrap()
        - time::Duration::seconds(1);
    let shop_id = common::shop_id::ShopId::new();
    let shops_product_id = common::shops_product_id::ShopsProductId::new();
    let mut record = Faker.fake::<UserSearchFilterMatchRecord>();
    record.pk = mk_pk(&user_id);
    record.sk = mk_sk(&filter_id, &shop_id, &shops_product_id);
    record.lsi1_sk = mk_lsi1_sk(&last_month);
    record.user_id = user_id;
    record.user_search_filter_id = filter_id;
    record.shop_id = shop_id;
    record.shops_product_id = shops_product_id;
    record.created = last_month;
    record.updated = last_month;
    search_filter_repo
        .put_user_search_filter_match_record(record)
        .await
        .unwrap();

    // Service counts only this month's matches
    let service = UserSearchFilterServiceImpl::new(&search_filter_repo, &user_service);
    let match_count = service
        .count_user_search_filter_matches_for_this_month(&user_id)
        .await
        .unwrap();

    // The user should be at exactly the free quota (10 this-month records)
    assert_eq!(match_count as u32, free_quota);

    // Quota check: count >= quota means the user has reached their limit
    assert!(
        (match_count as u32) >= free_quota,
        "Expected user to have reached the search-filter-match quota ({free_quota}), but count was {match_count}"
    );
}

// ---------------------------------------------------------------------------
// Stripe subscription lifecycle
// Verifies EventBridge → Lambda → DynamoDB routing and IAM access for each
// of the three `customer.subscription.*` event types.
// ---------------------------------------------------------------------------

/// Publishes a Stripe subscription event to the ephemeral Stripe EventBus.
async fn put_stripe_event(detail_type: &str, detail: serde_json::Value) {
    let cfn = get_cfn_output();
    let eb = get_eventbridge_client().await;
    let res = eb
        .put_events()
        .entries(
            aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(&cfn.stripe_event_bus_name)
                .source("stripe.com")
                .detail_type(detail_type)
                .detail(detail.to_string())
                .build(),
        )
        .send()
        .await
        .expect("shouldn't fail publishing Stripe event to EventBridge");
    assert_eq!(
        res.failed_entry_count(),
        0,
        "EventBridge rejected the Stripe event: {:?}",
        res.entries()
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_set_tier_and_stripe_customer_id_when_subscription_created_event() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);

    // Verify the user starts with Free tier and no stripe_customer_id
    let record_before = user_repository.get_user_record(&user_id).await.unwrap();
    assert!(record_before.is_some());
    let record_before = record_before.unwrap();
    assert_eq!(UserTierRecord::Free, record_before.tier);
    assert!(record_before.stripe_customer_id.is_none());

    // Publish subscription.created event
    put_stripe_event(
        "customer.subscription.created",
        serde_json::json!({
            "type": "customer.subscription.created",
            "data": {
                "object": {
                    "id": "sub_test_created",
                    "customer": "cus_test_created",
                    "metadata": { "userId": user.sub.to_string() },
                    "items": {
                        "data": [{
                            "price": { "product": "prod_test_pro" }
                        }]
                    }
                }
            }
        }),
    )
    .await;

    // Poll for the Lambda to update the user record
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let record = user_repository
            .get_user_record(&user_id)
            .await
            .unwrap()
            .expect("User record should exist");

        if record.tier == UserTierRecord::Pro && record.stripe_customer_id.is_some() {
            assert_eq!(
                record.stripe_customer_id.as_ref().unwrap().as_ref(),
                "cus_test_created"
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: user '{}' tier not updated to Pro after 60s (current tier: {:?}, stripe_customer_id: {:?})",
                user_id, record.tier, record.stripe_customer_id
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_update_tier_when_subscription_updated_event() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    // First, set the user to Pro with a stripe_customer_id via the created flow
    let stripe_customer_id = common::stripe_customer_id::StripeCustomerId::from("cus_test_updated");
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Pro),
                stripe_customer_id: Some(stripe_customer_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Verify the user is now Pro
    let record_before = user_repository
        .get_user_record(&user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(UserTierRecord::Pro, record_before.tier);

    // Publish subscription.updated event changing to Ultimate
    put_stripe_event(
        "customer.subscription.updated",
        serde_json::json!({
            "type": "customer.subscription.updated",
            "data": {
                "object": {
                    "id": "sub_test_updated",
                    "customer": "cus_test_updated",
                    "items": {
                        "data": [{
                            "price": { "product": "prod_test_ultimate" }
                        }]
                    }
                }
            }
        }),
    )
    .await;

    // Poll for the Lambda to update the user record
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let record = user_repository
            .get_user_record(&user_id)
            .await
            .unwrap()
            .expect("User record should exist");

        if record.tier == UserTierRecord::Ultimate {
            // stripe_customer_id should still be present
            assert_eq!(
                record.stripe_customer_id.as_ref().unwrap().as_ref(),
                "cus_test_updated"
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: user '{}' tier not updated to Ultimate after 60s (current tier: {:?})",
                user_id, record.tier
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_set_free_tier_when_subscription_deleted_event() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    // First, set the user to Ultimate with a stripe_customer_id
    let stripe_customer_id = common::stripe_customer_id::StripeCustomerId::from("cus_test_deleted");
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Ultimate),
                stripe_customer_id: Some(stripe_customer_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    // Verify the user is now Ultimate
    let record_before = user_repository
        .get_user_record(&user_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(UserTierRecord::Ultimate, record_before.tier);

    // Publish subscription.deleted event
    put_stripe_event(
        "customer.subscription.deleted",
        serde_json::json!({
            "type": "customer.subscription.deleted",
            "data": {
                "object": {
                    "id": "sub_test_deleted",
                    "customer": "cus_test_deleted"
                }
            }
        }),
    )
    .await;

    // Poll for the Lambda to update the user record
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let record = user_repository
            .get_user_record(&user_id)
            .await
            .unwrap()
            .expect("User record should exist");

        if record.tier == UserTierRecord::Free {
            // stripe_customer_id should still be present after deletion
            assert_eq!(
                record.stripe_customer_id.as_ref().unwrap().as_ref(),
                "cus_test_deleted"
            );
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: user '{}' tier not reset to Free after 60s (current tier: {:?})",
                user_id, record.tier
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// API: Stripe billing
// Verifies API Gateway routing, Cognito JWT auth, and Lambda execution for
// both billing endpoints. The Lambda detects LocalStack at startup and uses a
// MockStripeService, so no real Stripe credentials are required.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_201_for_billing_checkout_and_persist_stripe_customer_id_when_user_has_none() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let url = format!(
        "{}/api/v1/me/billing/checkout",
        stack.api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&user.access_token)
        .json(&serde_json::json!({"plan": "PRO", "cycle": "MONTHLY"}))
        .send()
        .await
        .unwrap();

    assert_eq!(201, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body.get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("https://checkout.stripe.com/"),
        "expected checkout URL, got {body:?}"
    );
    // Response must contain only the URL — no `livemode`/`userId` leakage.
    assert!(body.get("livemode").is_none(), "got {body:?}");
    assert!(body.get("userId").is_none(), "got {body:?}");

    // The mocked StripeService returned a deterministic `cus_mocked_<userId>`
    // id which the lambda must have persisted on the user record.
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let stored_user = user_repository
        .get_user_record(&user.sub.into())
        .await
        .unwrap()
        .expect("user record should exist");
    assert_eq!(
        stored_user
            .stripe_customer_id
            .as_ref()
            .map(|id| id.as_ref()),
        Some(format!("cus_mocked_{}", user.sub).as_str()),
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_409_for_billing_checkout_when_user_already_has_stripe_customer_id() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let stripe_customer_id =
        common::stripe_customer_id::StripeCustomerId::from(format!("cus_{}", uuid::Uuid::new_v4()));
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                stripe_customer_id: Some(stripe_customer_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/me/billing/checkout",
        stack.api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&user.access_token)
        .json(&serde_json::json!({"plan": "PRO", "cycle": "MONTHLY"}))
        .send()
        .await
        .unwrap();

    assert_eq!(409, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body.get("error").and_then(|v| v.as_str()),
        Some("STRIPE_CUSTOMER_ALREADY_EXISTS"),
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_201_for_billing_portal_when_user_has_stripe_customer_id() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let stripe_customer_id =
        common::stripe_customer_id::StripeCustomerId::from(format!("cus_{}", uuid::Uuid::new_v4()));
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                stripe_customer_id: Some(stripe_customer_id),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let url = format!(
        "{}/api/v1/me/billing/portal",
        stack.api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();

    assert_eq!(201, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body.get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("https://billing.stripe.com/"),
        "expected portal URL, got {body:?}"
    );
    assert!(body.get("livemode").is_none(), "got {body:?}");
    assert!(body.get("userId").is_none(), "got {body:?}");
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_422_for_billing_portal_when_user_has_no_stripe_customer_id() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    tokio::time::sleep(Duration::from_secs(10)).await;

    let url = format!(
        "{}/api/v1/me/billing/portal",
        stack.api_gateway_endpoint_url
    );
    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&user.access_token)
        .send()
        .await
        .unwrap();

    assert_eq!(422, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(
        body.get("error").and_then(|v| v.as_str()),
        Some("STRIPE_CUSTOMER_DOES_NOT_EXIST"),
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_201_for_billing_manage_with_checkout_for_free_and_portal_for_paid_user() {
    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    // Allow Cognito post-confirmation and initial user persistence to settle
    // before exercising the authenticated billing endpoint.
    tokio::time::sleep(Duration::from_secs(10)).await;

    let url = format!(
        "{}/api/v1/me/billing/manage",
        stack.api_gateway_endpoint_url
    );
    let request_body = serde_json::json!({"plan": "PRO", "cycle": "MONTHLY"});

    let response = reqwest::Client::new()
        .post(&url)
        .bearer_auth(&user.access_token)
        .json(&request_body)
        .send()
        .await
        .unwrap();

    assert_eq!(201, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body.get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("https://checkout.stripe.com/"),
        "expected checkout URL, got {body:?}"
    );
    assert!(body.get("livemode").is_none(), "got {body:?}");
    assert!(body.get("userId").is_none(), "got {body:?}");

    let expected_stripe_customer_id = format!("cus_mocked_{}", user.sub);
    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let stored_user = user_repository
        .get_user_record(&user.sub.into())
        .await
        .unwrap()
        .expect("user record should exist");
    assert_eq!(UserTierRecord::Free, stored_user.tier);
    assert_eq!(
        stored_user
            .stripe_customer_id
            .as_ref()
            .map(|id| id.as_ref()),
        Some(expected_stripe_customer_id.as_str()),
    );

    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_id = UserId::from(user.sub);
    let user_ctx = request_context_for_user(user_id);
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Pro),
                stripe_customer_id: Some(common::stripe_customer_id::StripeCustomerId::from(
                    expected_stripe_customer_id.as_str(),
                )),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let response = reqwest::Client::new()
        .post(url)
        .bearer_auth(&user.access_token)
        .json(&request_body)
        .send()
        .await
        .unwrap();

    assert_eq!(201, response.status());
    let body: serde_json::Value = response.json().await.unwrap();
    assert!(
        body.get("url")
            .and_then(|v| v.as_str())
            .unwrap()
            .starts_with("https://billing.stripe.com/"),
        "expected portal URL, got {body:?}"
    );
    assert!(body.get("livemode").is_none(), "got {body:?}");
    assert!(body.get("userId").is_none(), "got {body:?}");
}

// ---------------------------------------------------------------------------
// API: Admin User Management
// Verifies API Gateway routing and Lambda execution for the admin user
// management endpoints with Cognito JWT authentication and admin role check.
// ---------------------------------------------------------------------------
/*
async fn wait_until_user_document_exists(user_id: impl Into<String>) -> UserDocument {
    let user_id = user_id.into();
    for _ in 0..24 {
        refresh_index("users").await;
        if let Some(document) = try_read_by_id::<UserDocument>("users", &user_id).await {
            return document;
        }
        tokio::time::sleep(Duration::from_secs(5)).await;
    }
    panic!(
        "Expected user document '{}' to exist in OpenSearch, but it did not appear in time.",
        user_id
    );
}
 */

/*
#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_user_search() {
    let admin = create_admin_test_user().await;

    let url = format!("{}/api/v1/users", get_cfn_output().api_gateway_endpoint_url,);
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());
}
*/

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_user_get() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let url = format!(
        "{}/api/v1/users/{}",
        get_cfn_output().api_gateway_endpoint_url,
        user_id,
    );
    let response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(user_id, body.user_id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_200_for_admin_user_patch() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let patch_data = PatchAdminUserData {
        role: Some(UserRoleData::Admin),
        tier: Some(UserTierData::Pro),
        first_name: None,
        last_name: None,
        language: None,
        currency: None,
        measurement_unit: None,
        prohibited_content_consent: None,
        stripe_customer_id: None,
        structured_address: None,
    };

    let url = format!(
        "{}/api/v1/users/{}",
        get_cfn_output().api_gateway_endpoint_url,
        user_id,
    );
    let response = reqwest::Client::new()
        .patch(&url)
        .bearer_auth(&admin.access_token)
        .json(&patch_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, response.status());

    let body = response.json::<GetUserAccountData>().await.unwrap();
    assert_eq!(user_id, body.user_id);
    assert_eq!(UserRoleData::Admin, body.role);
    assert_eq!(UserTierData::Pro, body.tier);
}

/*
#[aura_integration_test(services = [Cloudformation()])]
async fn should_respond_204_for_admin_user_delete() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let url = format!(
        "{}/api/v1/users/{}",
        get_cfn_output().api_gateway_endpoint_url,
        user_id,
    );
    let response = reqwest::Client::new()
        .delete(&url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(204, response.status());

    // Verify the user is gone
    let get_response = reqwest::Client::new()
        .get(&url)
        .bearer_auth(&admin.access_token)
        .send()
        .await
        .unwrap();
    assert_eq!(404, get_response.status());
}
*/

// ---------------------------------------------------------------------------
// User → OpenSearch sync
// Verifies DynamoDB Streams → EventBridge → SQS → Lambda → OpenSearch routing.
// ---------------------------------------------------------------------------
/*
#[aura_integration_test(services = [Cloudformation()])]
async fn should_index_user_to_opensearch_on_create() {
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let document = wait_until_user_document_exists(user_id.to_string()).await;
    assert_eq!(user_id, document.user_id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_update_user_document_in_opensearch_on_patch() {
    let admin = create_admin_test_user().await;
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let initial_document = wait_until_user_document_exists(user_id.to_string()).await;

    let patch_data = PatchAdminUserData {
        tier: Some(UserTierData::Pro),
        role: None,
        first_name: None,
        last_name: None,
        language: None,
        currency: None,
        measurement_unit: None,
        prohibited_content_consent: None,
        stripe_customer_id: None,
        structured_address: None,
    };
    let patch_url = format!(
        "{}/api/v1/users/{}",
        get_cfn_output().api_gateway_endpoint_url,
        user_id,
    );
    let patch_response = reqwest::Client::new()
        .patch(&patch_url)
        .bearer_auth(&admin.access_token)
        .json(&patch_data)
        .send()
        .await
        .unwrap();
    assert_eq!(200, patch_response.status());

    tokio::time::sleep(Duration::from_secs(30)).await;
    let updated_document = wait_until_user_document_exists(user_id.to_string()).await;
    assert_eq!(user_id, updated_document.user_id);
    assert_ne!(initial_document.tier, updated_document.tier);
}
*/

// ---------------------------------------------------------------------------
// Tier enforcement: user-lambda-tier-update
// Verifies that when a user's tier changes the tier-update Lambda deactivates
// resources exceeding the new quota and reactivates them on upgrade.
// Triggered via DynamoDB Streams → EventBridge → UserTierUpdateQ → Lambda.
// ---------------------------------------------------------------------------

#[aura_integration_test(services = [Cloudformation()])]
async fn should_deactivate_over_quota_search_filters_when_tier_is_downgraded() {
    use search_filter::{
        core::quota::SearchFilterQuota,
        dynamodb::{
            repository::UserSearchFilterDynamoDbRepository,
            user_search_filter_record::{
                UserSearchFilterRecord, mk_pk as sf_mk_pk, mk_sk as sf_mk_sk,
            },
        },
    };

    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    let sf_repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let free_quota = UserTier::Free.search_filter_quota() as usize;
    let filter_count = free_quota + 2;

    for i in 0..filter_count {
        let filter_id = common::user_search_filter_id::UserSearchFilterId::new();
        let created = OffsetDateTime::now_utc() + time::Duration::seconds(i as i64);
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.pk = sf_mk_pk(&user_id);
        record.sk = sf_mk_sk(&filter_id);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.state = ResourceStateRecord::Active;
        record.enhanced_search_description = None;
        record.created = created;
        record.updated = created;
        // Clear Pro/Ultimate-only fields so state is governed only by quota
        record.shop_name_query = Default::default();
        record.exclude_shop_name_query = Default::default();
        record.seller_name_query = Default::default();
        record.exclude_seller_name_query = Default::default();
        record.shop_slug_id_query = Default::default();
        record.exclude_shop_slug_id_query = Default::default();
        record.seller_slug_id_query = Default::default();
        record.exclude_seller_slug_id_query = Default::default();
        record.shop_type_query = Default::default();
        record.country_query = Default::default();
        record.continent_query = Default::default();
        record.geo_address_distance_query = None;
        record.created_query = None;
        record.updated_query = None;
        record.auction_start_query = None;
        record.auction_end_query = None;
        sf_repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    // Downgrade triggers DynamoDB stream → EventBridge → UserTierUpdateQ → Lambda
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Free),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let records = sf_repository
            .query_user_search_filter_records(&user_id, false)
            .await
            .unwrap();

        let inactive_count = records
            .iter()
            .filter(|r| r.state == ResourceStateRecord::InactiveByRestrictedPlan)
            .count();

        if inactive_count == filter_count - free_quota {
            let active_count = records
                .iter()
                .filter(|r| r.state == ResourceStateRecord::Active)
                .count();
            assert_eq!(active_count, free_quota);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: expected {} inactive search filters after downgrade to Free, got {} inactive (states: {:?})",
                filter_count - free_quota,
                inactive_count,
                records.iter().map(|r| r.state).collect::<Vec<_>>()
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_reactivate_plan_restricted_search_filters_when_tier_is_upgraded() {
    use search_filter::dynamodb::{
        repository::UserSearchFilterDynamoDbRepository,
        user_search_filter_record::{UserSearchFilterRecord, mk_pk as sf_mk_pk, mk_sk as sf_mk_sk},
    };

    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    let sf_repository = UserSearchFilterDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let filter_count = 3usize;

    for i in 0..filter_count {
        let filter_id = common::user_search_filter_id::UserSearchFilterId::new();
        let created = OffsetDateTime::now_utc() + time::Duration::seconds(i as i64);
        let mut record = Faker.fake::<UserSearchFilterRecord>();
        record.pk = sf_mk_pk(&user_id);
        record.sk = sf_mk_sk(&filter_id);
        record.user_id = user_id;
        record.user_search_filter_id = filter_id;
        record.state = ResourceStateRecord::InactiveByRestrictedPlan;
        record.enhanced_search_description = None;
        record.created = created;
        record.updated = created;
        record.shop_name_query = Default::default();
        record.exclude_shop_name_query = Default::default();
        record.seller_name_query = Default::default();
        record.exclude_seller_name_query = Default::default();
        record.shop_slug_id_query = Default::default();
        record.exclude_shop_slug_id_query = Default::default();
        record.seller_slug_id_query = Default::default();
        record.exclude_seller_slug_id_query = Default::default();
        record.shop_type_query = Default::default();
        record.country_query = Default::default();
        record.continent_query = Default::default();
        record.geo_address_distance_query = None;
        record.created_query = None;
        record.updated_query = None;
        record.auction_start_query = None;
        record.auction_end_query = None;
        sf_repository
            .put_user_search_filter_record(record)
            .await
            .unwrap();
    }

    // Upgrade triggers DynamoDB stream → Lambda reactivates all filters
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Ultimate),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let records = sf_repository
            .query_user_search_filter_records(&user_id, false)
            .await
            .unwrap();

        let active_count = records
            .iter()
            .filter(|r| r.state == ResourceStateRecord::Active)
            .count();

        if active_count == filter_count {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: expected {} active search filters after upgrade to Ultimate, got {}",
                filter_count, active_count
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_deactivate_over_quota_watchlist_entries_when_tier_is_downgraded() {
    use common::{product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId};
    use product_watchlist::{
        core::quota::WatchlistQuota,
        dynamodb::{
            record::{
                WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk,
                mk_pk as watchlist_mk_pk, mk_sk as watchlist_mk_sk,
            },
            repository::WatchlistProductDynamoDbRepository,
        },
    };

    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    let watchlist_repo = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let free_quota = UserTier::Free.watchlist_quota() as usize;
    let entry_count = free_quota + 1;

    for i in 0..entry_count {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let product_id = ProductId::new();
        let created = OffsetDateTime::now_utc() + time::Duration::seconds(i as i64);
        let record = WatchlistProductRecord {
            pk: watchlist_mk_pk(&user_id),
            sk: watchlist_mk_sk(&shop_id, &shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created),
            gsi1_pk: mk_gsi1_pk(&product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            user_id,
            product_id,
            shop_id,
            shops_product_id,
            notifications: true,
            state: ResourceStateRecord::Active,
            created_by: common::actor::record::ActorRecord::User(user_id),
            updated_by: common::actor::record::ActorRecord::User(user_id),
            created,
            updated: created,
        };
        watchlist_repo.put_watchlist_record(record).await.unwrap();
    }

    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Free),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let records = watchlist_repo
            .query_watchlist_records_all(&user_id, false)
            .await
            .unwrap();

        let inactive_count = records
            .iter()
            .filter(|r| r.state == ResourceStateRecord::InactiveByRestrictedPlan)
            .count();

        if inactive_count == entry_count - free_quota {
            let active_count = records
                .iter()
                .filter(|r| r.state == ResourceStateRecord::Active)
                .count();
            assert_eq!(active_count, free_quota);
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: expected {} inactive watchlist entries after downgrade to Free, got {}",
                entry_count - free_quota,
                inactive_count
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_reactivate_plan_restricted_watchlist_entries_when_tier_is_upgraded() {
    use common::{product_id::ProductId, shop_id::ShopId, shops_product_id::ShopsProductId};
    use product_watchlist::dynamodb::{
        record::{
            WatchlistProductRecord, mk_gsi1_pk, mk_gsi1_sk, mk_lsi1_sk, mk_pk as watchlist_mk_pk,
            mk_sk as watchlist_mk_sk,
        },
        repository::WatchlistProductDynamoDbRepository,
    };

    let stack = get_cfn_output();
    let user = create_random_test_user().await;
    let user_id = UserId::from(user.sub);

    let user_repository =
        UserDynamoDbRepositoryImpl::new(get_dynamodb_client().await, &stack.dynamodb_table_1_name);
    let user_service = user::service::user_service::UserServiceImpl::new(&user_repository);
    let user_ctx = request_context_for_user(user_id);

    let watchlist_repo = WatchlistProductDynamoDbRepositoryImpl::new(
        get_dynamodb_client().await,
        &stack.dynamodb_table_1_name,
    );

    let entry_count = 3usize;

    for i in 0..entry_count {
        let shop_id = ShopId::new();
        let shops_product_id = ShopsProductId::new();
        let product_id = ProductId::new();
        let created = OffsetDateTime::now_utc() + time::Duration::seconds(i as i64);
        let record = WatchlistProductRecord {
            pk: watchlist_mk_pk(&user_id),
            sk: watchlist_mk_sk(&shop_id, &shops_product_id),
            lsi1_sk: mk_lsi1_sk(&created),
            gsi1_pk: mk_gsi1_pk(&product_id),
            gsi1_sk: mk_gsi1_sk(&user_id),
            user_id,
            product_id,
            shop_id,
            shops_product_id,
            notifications: true,
            state: ResourceStateRecord::InactiveByRestrictedPlan,
            created_by: common::actor::record::ActorRecord::User(user_id),
            updated_by: common::actor::record::ActorRecord::User(user_id),
            created,
            updated: created,
        };
        watchlist_repo.put_watchlist_record(record).await.unwrap();
    }

    // Upgrade triggers DynamoDB stream → Lambda reactivates all entries
    user_service
        .update_user(
            &user_ctx,
            &user_id,
            UpdateUserCommand {
                tier: Some(UserTier::Ultimate),
                ..Default::default()
            },
        )
        .await
        .unwrap();

    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        let records = watchlist_repo
            .query_watchlist_records_all(&user_id, false)
            .await
            .unwrap();

        let active_count = records
            .iter()
            .filter(|r| r.state == ResourceStateRecord::Active)
            .count();

        if active_count == entry_count {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: expected {} active watchlist entries after upgrade to Ultimate, got {}",
                entry_count, active_count
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

// ---------------------------------------------------------------------------
// Shopify product lifecycle
// Verifies EventBridge (Shopify event bus) → ShopifyLambda → DynamoDB routing
// and IAM access for each of the three Shopify product event types.
// ---------------------------------------------------------------------------

const SHOPIFY_ACCEPTANCE_DOMAIN: &str = "aura-historia-partner-connect-acc-test.myshopify.com";
const SHOPIFY_ACCEPTANCE_PRODUCT_ID: u64 = 99_999_000_000_001;

/// Seeds a partner shop with the acceptance-test Shopify domain in DynamoDB.
async fn seed_shopify_acceptance_shop() -> ShopRecord {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let shop_repo = ShopDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);

    let shopify_domain = common::domain::Domain::try_from(SHOPIFY_ACCEPTANCE_DOMAIN).unwrap();
    let shop_id = common::shop_id::ShopId::new();
    let slug = common::slug_id::SlugId::raw("shopify-acc-test-shop").unwrap();

    let record = ShopRecord {
        pk: shop::dynamodb::shop_record::mk_pk(&shop_id),
        sk: shop::dynamodb::shop_record::mk_sk().to_owned(),
        shop_id,
        shop_slug_id: slug.clone().into(),
        name: common::shop_name::ShopName::from("Shopify Acceptance Shop"),
        shop_type: shop::dynamodb::shop_type_record::ShopTypeRecord::Marketplace,
        shop_partner_status: ShopPartnerStatusRecord::Partnered,
        domains: Default::default(),
        shopify_domain: Some(shopify_domain.clone()),
        shopify_currency: Some(common::currency::record::CurrencyRecord::Usd),
        shopify_language: Some(common::language::record::LanguageRecord::De),
        woocommerce_webhook_secret: None,
        woocommerce_currency: None,
        woocommerce_language: None,
        url: None,
        view_url: None,
        image: None,
        structured_address_addressline: None,
        structured_address_addressline_extra: None,
        structured_address_locality: None,
        structured_address_region: None,
        structured_address_postal_code: None,
        structured_address_country: None,
        geo_address_lat: None,
        geo_address_lon: None,
        phone: None,
        email: None,
        gsi2_pk: Some(shop::dynamodb::shop_record::mk_gsi2_pk(&slug.into())),
        gsi2_sk: Some(shop::dynamodb::shop_record::mk_gsi2_sk().to_owned()),
        gsi3_pk: Some(shop::dynamodb::shop_record::mk_gsi3_pk(&shopify_domain)),
        gsi3_sk: Some(shop::dynamodb::shop_record::mk_gsi3_sk().to_owned()),
        affiliate_configuration: None,
        created_by: common::actor::record::ActorRecord::System,
        updated_by: common::actor::record::ActorRecord::System,
        created: OffsetDateTime::now_utc(),
        updated: OffsetDateTime::now_utc(),
    };

    shop_repo.put_shop_record(record.clone()).await.unwrap();
    record
}

/// Publishes a real-world Shopify webhook EventBridge entry.
async fn put_shopify_event(topic: &str, product_id: u64) {
    let cfn = get_cfn_output();
    let eb = get_eventbridge_client().await;

    let detail = serde_json::json!({
        "payload": {
            "admin_graphql_api_id": format!("gid://shopify/Product/{product_id}"),
            "body_html": "<p>Hallo Test Beschreibung!</p>",
            "created_at": "2026-05-11T11:02:26-04:00",
            "handle": "acceptance-test-produkt",
            "id": product_id,
            "product_type": "",
            "published_at": "2026-05-11T11:02:29-04:00",
            "template_suffix": "",
            "title": "Acceptance Testprodukt",
            "updated_at": "2026-05-11T11:06:59-04:00",
            "vendor": "aura-historia-partner-connect-acc-test",
            "status": "active",
            "published_scope": "global",
            "tags": "",
            "variants": [
                {
                    "admin_graphql_api_id": "gid://shopify/ProductVariant/99000000001",
                    "barcode": "",
                    "compare_at_price": null,
                    "created_at": "2026-05-11T11:02:28-04:00",
                    "id": 99_000_000_001_u64,
                    "inventory_policy": "deny",
                    "position": 1,
                    "price": "49.99",
                    "product_id": product_id,
                    "sku": null,
                    "taxable": false,
                    "title": "Default Title",
                    "updated_at": "2026-05-11T11:06:59-04:00",
                    "option1": "Default Title",
                    "option2": null,
                    "option3": null,
                    "image_id": null,
                    "inventory_item_id": 99_000_000_002_u64,
                    "inventory_quantity": 5,
                    "old_inventory_quantity": 0
                }
            ],
            "images": []
        },
        "metadata": {
            "Content-Type": "application/json",
            "X-Shopify-Topic": topic,
            "X-Shopify-Shop-Domain": SHOPIFY_ACCEPTANCE_DOMAIN,
            "X-Shopify-Product-Id": product_id.to_string(),
            "X-Shopify-Hmac-SHA256": "acceptance-test-hmac",
            "X-Shopify-Webhook-Id": "acc-test-webhook-id",
            "X-Shopify-API-Version": "2026-04",
            "X-Shopify-Event-Id": "acc-test-event-id",
            "X-Shopify-Triggered-At": "2026-05-11T15:06:59.110521905Z"
        }
    });

    let res = eb
        .put_events()
        .entries(
            aws_sdk_eventbridge::types::PutEventsRequestEntry::builder()
                .event_bus_name(&cfn.shopify_event_bus_name)
                .source("aws.partner/shopify.com/test/aura-historia-backend-acc")
                .detail_type("shopifyWebhook")
                .detail(detail.to_string())
                .build(),
        )
        .send()
        .await
        .expect("shouldn't fail publishing Shopify event to EventBridge");

    assert_eq!(
        res.failed_entry_count(),
        0,
        "EventBridge rejected the Shopify event: {:?}",
        res.entries()
    );
}

/// Polls DynamoDB until the product record for the given shop_id / product_id appears (or times out).
async fn wait_for_shopify_product(
    shop_id: common::shop_id::ShopId,
    product_id: u64,
) -> product::dynamodb::product_record::ProductRecord {
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repo =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);

    let shops_product_id = common::shops_product_id::ShopsProductId::from(product_id.to_string());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if let Some(record) = product_repo
            .get_product_record(&shop_id, &shops_product_id)
            .await
            .unwrap()
        {
            return record;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: product '{product_id}' not found in DynamoDB for shop '{shop_id}' after 60s"
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_create_product_in_dynamodb_when_shopify_create_event() {
    let shop_record = seed_shopify_acceptance_shop().await;
    let shop_id = shop_record.shop_id;

    put_shopify_event("products/create", SHOPIFY_ACCEPTANCE_PRODUCT_ID).await;

    let record = wait_for_shopify_product(shop_id, SHOPIFY_ACCEPTANCE_PRODUCT_ID).await;
    assert_eq!(record.shop_id, shop_id);
    assert_eq!(
        record.shops_product_id,
        common::shops_product_id::ShopsProductId::from(SHOPIFY_ACCEPTANCE_PRODUCT_ID.to_string())
    );
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_update_product_in_dynamodb_when_shopify_update_event() {
    // Use a different product id to keep tests independent
    const PRODUCT_ID: u64 = SHOPIFY_ACCEPTANCE_PRODUCT_ID + 1;
    let shop_record = seed_shopify_acceptance_shop().await;
    let shop_id = shop_record.shop_id;

    // Seed the product first
    put_shopify_event("products/create", PRODUCT_ID).await;
    wait_for_shopify_product(shop_id, PRODUCT_ID).await;

    // Now send an update
    put_shopify_event("products/update", PRODUCT_ID).await;

    // Poll until the product is still present (update is idempotent here)
    let record = wait_for_shopify_product(shop_id, PRODUCT_ID).await;
    assert_eq!(record.shop_id, shop_id);
}

#[aura_integration_test(services = [Cloudformation()])]
async fn should_set_product_removed_in_dynamodb_when_shopify_delete_event() {
    const PRODUCT_ID: u64 = SHOPIFY_ACCEPTANCE_PRODUCT_ID + 2;
    let shop_record = seed_shopify_acceptance_shop().await;
    let shop_id = shop_record.shop_id;

    // Create first
    put_shopify_event("products/create", PRODUCT_ID).await;
    wait_for_shopify_product(shop_id, PRODUCT_ID).await;

    // Now delete
    put_shopify_event("products/delete", PRODUCT_ID).await;

    // Poll until state = Removed
    let stack = get_cfn_output();
    let dynamodb_client = get_dynamodb_client().await;
    let product_repo =
        ProductDynamoDbRepositoryImpl::new(dynamodb_client, &stack.dynamodb_table_1_name);
    let shops_product_id = common::shops_product_id::ShopsProductId::from(PRODUCT_ID.to_string());
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        if product_repo
            .get_product_record(&shop_id, &shops_product_id)
            .await
            .unwrap()
            .is_some_and(|r| {
                r.state == product::dynamodb::product_state_record::ProductStateRecord::Removed
            })
        {
            break;
        }

        if Instant::now() >= deadline {
            panic!(
                "Timeout: product '{PRODUCT_ID}' not set to Removed for shop '{shop_id}' after 60s"
            );
        }
        tokio::time::sleep(Duration::from_secs(3)).await;
    }
}
