domain_primitives::object_id_newtype!(ContentNoticeId, "ntc");

#[cfg(test)]
mod tests {
    use super::ContentNoticeId;
    use domain_primitives::object_id::ObjectIdError;
    use std::error::Error;
    use uuid::Uuid;

    const UUID_TEXT: &str = "01890a5d-ac96-774b-bf1d-d5586c639f75";
    const TYPE_ID_SUFFIX: &str = "01h455vb4pex5vy7enb1p677vn";

    #[test]
    fn should_round_trip_canonical_ntc_type_id() -> Result<(), Box<dyn Error>> {
        let uuid = Uuid::parse_str(UUID_TEXT)?;
        let id = ContentNoticeId::try_from(uuid)?;

        assert_eq!("ntc", ContentNoticeId::PREFIX);
        assert_eq!(format!("ntc_{TYPE_ID_SUFFIX}"), id.to_string());
        assert_eq!(Ok(id), ContentNoticeId::try_from(id.to_string()));
        assert_eq!(uuid, id.into_uuid());
        Ok(())
    }

    #[test]
    fn should_generate_uuid_v7_content_notice_id() {
        let id = ContentNoticeId::new();

        assert_eq!(7, id.as_uuid().get_version_num());
        assert_eq!(Ok(id), ContentNoticeId::try_from(id.to_string()));
    }

    #[test]
    fn should_reject_wrong_prefix() {
        assert!(matches!(
            ContentNoticeId::try_from(format!("ntf_{TYPE_ID_SUFFIX}")),
            Err(ObjectIdError::WrongPrefix {
                expected: "ntc",
                ..
            })
        ));
    }

    #[test]
    fn should_reject_bare_uuid() {
        assert!(ContentNoticeId::try_from(UUID_TEXT).is_err());
    }

    #[test]
    fn should_reject_non_v7_uuid() -> Result<(), Box<dyn Error>> {
        let v4 = Uuid::parse_str("2b6f0cc9-04f8-4c0e-8f5c-2f1a8a3f5e10")?;

        assert_eq!(
            Err(ObjectIdError::UnsupportedUuidVersion { actual: 4 }),
            ContentNoticeId::try_from(v4)
        );
        Ok(())
    }
}
