use common::error::boxed::BoxError;
use notification_service::ports::notification_channel_sender::NotificationChannelSendError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ProviderFailure {
    Retryable { code: &'static str },
    Permanent { code: &'static str },
}

impl ProviderFailure {
    pub(crate) fn into_send_error(
        self,
        source: impl Into<BoxError>,
    ) -> NotificationChannelSendError {
        match self {
            Self::Retryable { code } => NotificationChannelSendError::Retryable {
                code,
                source: source.into(),
            },
            Self::Permanent { code } => NotificationChannelSendError::Permanent {
                code,
                source: source.into(),
            },
        }
    }
}

pub(crate) fn classify_s3_template_fetch(
    template_missing: bool,
    status_code: Option<u16>,
) -> ProviderFailure {
    if template_missing || matches!(status_code, Some(404)) {
        ProviderFailure::Permanent {
            code: "S3_TEMPLATE_MISSING",
        }
    } else if matches!(status_code, Some(400..=499) if status_code != Some(408) && status_code != Some(429))
    {
        ProviderFailure::Permanent {
            code: "S3_TEMPLATE_ACCESS_OR_CONFIG_INVALID",
        }
    } else {
        ProviderFailure::Retryable {
            code: "S3_TEMPLATE_FETCH_RETRYABLE",
        }
    }
}

pub(crate) fn classify_ses_send(
    throttled: bool,
    permanently_rejected: bool,
    status_code: Option<u16>,
) -> ProviderFailure {
    if throttled || matches!(status_code, Some(429)) {
        ProviderFailure::Retryable {
            code: "SES_THROTTLED",
        }
    } else if permanently_rejected || matches!(status_code, Some(400..=499)) {
        ProviderFailure::Permanent {
            code: "SES_REQUEST_OR_CONFIGURATION_INVALID",
        }
    } else {
        ProviderFailure::Retryable {
            code: "SES_SEND_RETRYABLE",
        }
    }
}

pub(crate) fn provider_error(
    failure: ProviderFailure,
    source: impl Into<BoxError>,
) -> NotificationChannelSendError {
    failure.into_send_error(source)
}

#[cfg(test)]
mod tests {
    use super::*;
    use common::error::boxed::box_error;
    use rstest::rstest;

    #[rstest]
    #[case(true, None, ProviderFailure::Permanent { code: "S3_TEMPLATE_MISSING" })]
    #[case(false, Some(403), ProviderFailure::Permanent { code: "S3_TEMPLATE_ACCESS_OR_CONFIG_INVALID" })]
    #[case(false, Some(408), ProviderFailure::Retryable { code: "S3_TEMPLATE_FETCH_RETRYABLE" })]
    #[case(false, Some(500), ProviderFailure::Retryable { code: "S3_TEMPLATE_FETCH_RETRYABLE" })]
    fn should_classify_s3_template_fetch_failures(
        #[case] template_missing: bool,
        #[case] status_code: Option<u16>,
        #[case] expected: ProviderFailure,
    ) {
        assert_eq!(
            expected,
            classify_s3_template_fetch(template_missing, status_code)
        );
    }

    #[rstest]
    #[case(true, false, None, ProviderFailure::Retryable { code: "SES_THROTTLED" })]
    #[case(false, true, None, ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(false, false, Some(400), ProviderFailure::Permanent { code: "SES_REQUEST_OR_CONFIGURATION_INVALID" })]
    #[case(false, false, Some(503), ProviderFailure::Retryable { code: "SES_SEND_RETRYABLE" })]
    fn should_classify_ses_send_failures(
        #[case] throttled: bool,
        #[case] permanently_rejected: bool,
        #[case] status_code: Option<u16>,
        #[case] expected: ProviderFailure,
    ) {
        assert_eq!(
            expected,
            classify_ses_send(throttled, permanently_rejected, status_code)
        );
    }

    #[test]
    fn should_not_include_provider_payload_in_error_display() {
        let error = provider_error(
            ProviderFailure::Permanent {
                code: "SES_REQUEST_OR_CONFIGURATION_INVALID",
            },
            box_error(std::io::Error::other(
                "recipient@example.test provider payload",
            )),
        );

        assert_eq!(
            "notification channel send failed permanently: SES_REQUEST_OR_CONFIGURATION_INVALID",
            error.to_string()
        );
        assert!(!error.to_string().contains("recipient@example.test"));
    }
}
