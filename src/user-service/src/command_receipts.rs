use crate::use_cases::queries::get_access_token::AccessTokenView;
use crate::{
    ports::UserDetailsView,
    use_cases::commands::{
        change_user_role::ChangeUserRoleResult, change_user_tier::ChangeUserTierResult,
        update_access_token::UpdateAccessTokenResult, update_user_profile::UpdateUserProfileResult,
    },
};
use application::{
    error::{BoxError, static_error},
    idempotency::{self as retry, CommandOutcome},
};
use serde_json::{Value, json};
impl CommandOutcome for UserDetailsView {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"user_id":self.user_id,"email":self.email,"first_name":self.first_name,"last_name":self.last_name,"language":self.language.map(|v|v.as_str()),"currency":self.currency.map(|v|v.as_str()),"measurement_unit":self.measurement_unit.map(|v|v.as_str()),"show_unassessed_or_sensitive_content":self.show_unassessed_or_sensitive_content,"marketing_email_consent":self.marketing_email_consent,"tier":self.tier.as_str(),"role":self.role.as_str(),"stripe_customer_id":self.stripe_customer_id}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            user_id: retry::field(&v, "user_id")?,
            email: retry::field(&v, "email")?,
            first_name: retry::field(&v, "first_name")?,
            last_name: retry::field(&v, "last_name")?,
            language: code(&v, "language", localization::Language::from_code)?,
            currency: code(&v, "currency", money::Currency::from_code)?,
            measurement_unit: code(
                &v,
                "measurement_unit",
                user_core::measurement_unit::MeasurementUnit::from_code,
            )?,
            show_unassessed_or_sensitive_content: retry::field(
                &v,
                "show_unassessed_or_sensitive_content",
            )?,
            marketing_email_consent: retry::field(&v, "marketing_email_consent")?,
            tier: user_core::tier::UserTier::from_code(&retry::field::<String>(&v, "tier")?)
                .ok_or_else(|| static_error("invalid tier"))?,
            role: user_core::role::UserRole::from_code(&retry::field::<String>(&v, "role")?)
                .ok_or_else(|| static_error("invalid role"))?,
            stripe_customer_id: retry::field(&v, "stripe_customer_id")?,
        })
    }
}
fn code<T>(v: &Value, k: &str, parse: fn(&str) -> Option<T>) -> Result<Option<T>, BoxError> {
    retry::field::<Option<String>>(v, k)?
        .map(|s| parse(&s).ok_or_else(|| static_error("invalid outcome code")))
        .transpose()
}
macro_rules! view_result {
    ($t:ty,$view:ty) => {
        impl CommandOutcome for $t {
            fn private_owner(&self) -> Option<user_core::user_id::UserId> {
                self.view.private_owner()
            }
            fn encode(&self) -> Result<Value, BoxError> {
                self.view.encode()
            }
            fn decode(v: Value) -> Result<Self, BoxError> {
                Ok(Self {
                    view: <$view>::decode(v)?,
                })
            }
        }
    };
}
view_result!(UpdateUserProfileResult, UserDetailsView);
view_result!(ChangeUserRoleResult, UserDetailsView);
view_result!(ChangeUserTierResult, UserDetailsView);
view_result!(UpdateAccessTokenResult, AccessTokenView);
impl CommandOutcome for AccessTokenView {
    fn private_owner(&self) -> Option<user_core::user_id::UserId> {
        Some(self.user_id)
    }
    fn encode(&self) -> Result<Value, BoxError> {
        Ok(
            json!({"user_id":self.user_id,"access_token_id":self.access_token_id,"name":self.name.as_ref(),"scopes":scopes(&self.scopes),"origin":origin(&self.origin),"expires":self.expires.map(retry::time_value).transpose()?}),
        )
    }
    fn decode(v: Value) -> Result<Self, BoxError> {
        Ok(Self {
            user_id: retry::field(&v, "user_id")?,
            access_token_id: retry::field(&v, "access_token_id")?,
            name: {
                let s: String = retry::field(&v, "name")?;
                if s.len() > 128 {
                    return Err(static_error("invalid token name"));
                }
                s.into()
            },
            scopes: decode_scopes(&v, "scopes")?,
            origin: decode_origin(retry::field(&v, "origin")?)?,
            expires: retry::field::<Option<String>>(&v, "expires")?
                .map(|s| {
                    time::OffsetDateTime::parse(&s, &time::format_description::well_known::Rfc3339)
                })
                .transpose()?,
        })
    }
}
pub(crate) fn scopes(v: &std::collections::HashSet<user_core::access_token::Scope>) -> Value {
    let mut a = v.iter().map(ToString::to_string).collect::<Vec<_>>();
    a.sort_unstable();
    json!(a)
}

pub(crate) fn origin(v: &user_core::access_token::AccessTokenOrigin) -> Value {
    use user_core::access_token::AccessTokenOrigin;
    match v {
        AccessTokenOrigin::User => json!({"kind":"USER"}),
        AccessTokenOrigin::OAuth { client_id } => json!({"kind":"OAUTH","client_id":client_id}),
    }
}
fn decode_origin(v: Value) -> Result<user_core::access_token::AccessTokenOrigin, BoxError> {
    use user_core::access_token::AccessTokenOrigin;
    match retry::field::<String>(&v, "kind")?.as_str() {
        "USER" => Ok(AccessTokenOrigin::User),
        "OAUTH" => Ok(AccessTokenOrigin::OAuth {
            client_id: retry::field(&v, "client_id")?,
        }),
        _ => Err(static_error("invalid token origin")),
    }
}
fn decode_scopes(
    v: &Value,
    k: &str,
) -> Result<std::collections::HashSet<user_core::access_token::Scope>, BoxError> {
    retry::field::<Vec<String>>(v, k)?
        .into_iter()
        .map(|v| {
            user_core::access_token::Scope::from_code(&v)
                .ok_or_else(|| static_error("invalid scope"))
        })
        .collect()
}

application::command_outcome!(
    crate::use_cases::commands::suspend_user::SuspendUserResult { user_id, suspended }
);
application::command_outcome!(
    crate::use_cases::commands::unsuspend_user::UnsuspendUserResult { user_id, suspended }
);
application::command_outcome!(crate::use_cases::commands::delete_user::DeleteUserResult {
    user_id
});

pub(crate) struct IssuedAccessToken {
    pub access_token_id: user_core::access_token::AccessTokenId,
    pub user_id: user_core::user_id::UserId,
}
application::command_outcome!(IssuedAccessToken {
    access_token_id,
    user_id
});
