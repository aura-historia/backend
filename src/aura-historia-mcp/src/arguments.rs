//! JSON argument decoding with raw containers and the SDK's scalar number semantics.
use serde::{
    Deserialize, Deserializer,
    de::{
        DeserializeOwned, DeserializeSeed, EnumAccess, IntoDeserializer, MapAccess, VariantAccess,
        Visitor,
        value::{MapDeserializer, SeqDeserializer},
    },
};
use serde_json::{Error, Value, value::RawValue};
use std::fmt;

pub(crate) fn decode<T: DeserializeOwned>(raw: Box<RawValue>) -> Result<T, Error> {
    // RawValue containers defer their children. Check the complete JSON first so that
    // deferral cannot reset serde_json's recursion limit at every nested container.
    let _: Value = serde_json::from_str(raw.get())?;
    T::deserialize(Argument(&raw))
}

struct Argument<'de>(&'de RawValue);
impl<'de> IntoDeserializer<'de, Error> for Argument<'de> {
    type Deserializer = Self;
    fn into_deserializer(self) -> Self {
        self
    }
}

struct Object<'de>(Vec<(String, Argument<'de>)>);
impl<'de> Deserialize<'de> for Object<'de> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct ObjectVisitor;
        impl<'de> Visitor<'de> for ObjectVisitor {
            type Value = Object<'de>;
            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("an object")
            }
            fn visit_map<M: MapAccess<'de>>(self, mut map: M) -> Result<Object<'de>, M::Error> {
                let mut entries = Vec::new();
                while let Some((key, raw)) = map.next_entry::<String, &'de RawValue>()? {
                    // Number's internal map visits arise only from scalar decoding below.
                    // A caller object must not become a scalar inside an opaque Value.
                    if key == "$serde_json::private::Number" {
                        return Err(serde::de::Error::custom(
                            "private JSON number representation is not an argument object",
                        ));
                    }
                    entries.push((key, Argument(raw)));
                }
                Ok(Object(entries))
            }
        }
        deserializer.deserialize_map(ObjectVisitor)
    }
}

// Typed scalar reads use the JSON parser directly, including its rejection of objects
// masquerading as serde_json's private number representation.
macro_rules! scalar {
    ($($method:ident),* $(,)?) => {$(
        fn $method<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
            Deserializer::$method(&mut serde_json::Deserializer::from_str(self.0.get()), visitor)
        }
    )*};
}

impl<'de> Deserializer<'de> for Argument<'de> {
    type Error = Error;

    fn deserialize_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        match self.0.get().as_bytes().first() {
            Some(b'{') => {
                let object: Object = serde_json::from_str(self.0.get())?;
                let mut entries = MapDeserializer::new(object.0.into_iter());
                let value = visitor.visit_map(&mut entries)?;
                entries.end()?;
                Ok(value)
            }
            Some(b'[') => {
                let items: Vec<&'de RawValue> = serde_json::from_str(self.0.get())?;
                let mut items = SeqDeserializer::new(items.into_iter().map(Argument));
                let value = visitor.visit_seq(&mut items)?;
                items.end()?;
                Ok(value)
            }
            _ => {
                // With arbitrary_precision, from_str's deserialize_any hands fractional
                // numbers to Serde flatten buffers as maps. Value/Number instead uses
                // native numeric visits where lossless and retains exact other numbers.
                serde_json::from_str::<Value>(self.0.get())?.deserialize_any(visitor)
            }
        }
    }

    scalar! {
        deserialize_bool, deserialize_i8, deserialize_i16, deserialize_i32, deserialize_i64,
        deserialize_i128, deserialize_u8, deserialize_u16, deserialize_u32, deserialize_u64,
        deserialize_u128, deserialize_f32, deserialize_f64, deserialize_char, deserialize_str,
        deserialize_string, deserialize_bytes, deserialize_byte_buf, deserialize_identifier
    }

    fn deserialize_option<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        if self.0.get() == "null" {
            visitor.visit_none()
        } else {
            visitor.visit_some(self)
        }
    }

    fn deserialize_newtype_struct<V: Visitor<'de>>(
        self,
        name: &'static str,
        visitor: V,
    ) -> Result<V::Value, Error> {
        // In particular, RawValue must receive the original JSON, including duplicate
        // item fields which asynchronous admission rejects independently.
        Deserializer::deserialize_newtype_struct(
            &mut serde_json::Deserializer::from_str(self.0.get()),
            name,
            visitor,
        )
    }

    fn deserialize_enum<V: Visitor<'de>>(
        self,
        name: &'static str,
        variants: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        if self.0.get().starts_with('{') {
            let mut entries = serde_json::from_str::<Object>(self.0.get())?.0.into_iter();
            let Some((variant, value)) = entries.next() else {
                return Err(serde::de::Error::custom("expected one enum variant"));
            };
            if entries.next().is_some() {
                return Err(serde::de::Error::custom("expected one enum variant"));
            }
            visitor.visit_enum(ArgumentEnum { variant, value })
        } else {
            Deserializer::deserialize_enum(
                &mut serde_json::Deserializer::from_str(self.0.get()),
                name,
                variants,
                visitor,
            )
        }
    }

    fn deserialize_ignored_any<V: Visitor<'de>>(self, visitor: V) -> Result<V::Value, Error> {
        visitor.visit_unit()
    }

    serde::forward_to_deserialize_any! {
        unit unit_struct seq tuple tuple_struct map struct
    }
}

struct ArgumentEnum<'de> {
    variant: String,
    value: Argument<'de>,
}
impl<'de> EnumAccess<'de> for ArgumentEnum<'de> {
    type Error = Error;
    type Variant = Argument<'de>;
    fn variant_seed<V: DeserializeSeed<'de>>(
        self,
        seed: V,
    ) -> Result<(V::Value, Argument<'de>), Error> {
        Ok((
            seed.deserialize(self.variant.into_deserializer())?,
            self.value,
        ))
    }
}
impl<'de> VariantAccess<'de> for Argument<'de> {
    type Error = Error;
    fn unit_variant(self) -> Result<(), Error> {
        Deserialize::deserialize(self)
    }
    fn newtype_variant_seed<T: DeserializeSeed<'de>>(self, seed: T) -> Result<T::Value, Error> {
        seed.deserialize(self)
    }
    fn tuple_variant<V: Visitor<'de>>(self, _: usize, visitor: V) -> Result<V::Value, Error> {
        self.deserialize_seq(visitor)
    }
    fn struct_variant<V: Visitor<'de>>(
        self,
        _: &'static [&'static str],
        visitor: V,
    ) -> Result<V::Value, Error> {
        self.deserialize_map(visitor)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Debug, Deserialize)]
    struct Numbers {
        coordinate: f64,
        amount: u64,
        cursor: Value,
    }
    #[derive(Debug, Deserialize)]
    struct Flattened {
        #[serde(flatten)]
        numbers: Numbers,
    }
    fn raw(value: &str) -> Box<RawValue> {
        RawValue::from_string(value.to_owned()).unwrap()
    }

    #[test]
    fn should_decode_flattened_fractions_without_rounding_integers_or_opaque_numbers() {
        let decimal = "0.12345678901234567890123456789";
        let input = format!(
            r#"{{"coordinate":52.125,"amount":18446744073709551615,"cursor":{{"exact":{decimal}}}}}"#
        );
        let value: Flattened = decode(raw(&input)).unwrap();
        assert_eq!(value.numbers.coordinate, 52.125);
        assert_eq!(value.numbers.amount, u64::MAX);
        assert_eq!(value.numbers.cursor["exact"].to_string(), decimal);
        assert!(
            decode::<Flattened>(raw(
                &input.replace("18446744073709551615", "18446744073709551616")
            ))
            .is_err()
        );
    }

    #[test]
    fn should_reject_duplicate_fields_and_caller_supplied_private_number_maps() {
        for input in [
            r#"{"coordinate":52.125,"coordinate":53.125,"amount":1,"cursor":null}"#,
            r#"{"coordinate":{"$serde_json::private::Number":"52.125"},"amount":1,"cursor":null}"#,
            r#"{"coordinate":52.125,"amount":1,"cursor":{"nested":{"$serde_json::private::Number":"52.125"}}}"#,
        ] {
            assert!(decode::<Numbers>(raw(input)).is_err());
            assert!(decode::<Flattened>(raw(input)).is_err());
        }
        assert!(decode::<Value>(raw(r#"{"$serde_json::private::Number":"52.125"}"#)).is_err());
    }

    #[test]
    fn should_keep_the_complete_json_recursion_limit_before_deferred_decoding() {
        let input = format!("{}null{}", "[".repeat(130), "]".repeat(130));
        let error = decode::<Value>(raw(&input)).unwrap_err();
        assert!(error.is_syntax());
        assert!(error.to_string().contains("recursion limit"));
    }
}
