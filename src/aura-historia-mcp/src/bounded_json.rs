use serde::Serialize;
use std::io::{self, Write};

pub(crate) fn serialize<T: Serialize>(
    value: &T,
    limit: usize,
) -> Result<Vec<u8>, SerializationError> {
    let mut writer = BoundedWriter {
        bytes: Vec::new(),
        limit,
        exceeded: false,
    };
    match serde_json::to_writer(&mut writer, value) {
        Ok(()) => Ok(writer.bytes),
        Err(_) if writer.exceeded => Err(SerializationError::Limit),
        Err(source) => Err(SerializationError::Serialize(source)),
    }
}

#[derive(Debug, thiserror::Error)]
pub(crate) enum SerializationError {
    #[error("JSON size limit exceeded")]
    Limit,
    #[error("JSON serialization failed")]
    Serialize(#[source] serde_json::Error),
}

impl SerializationError {
    pub(crate) fn into_operation(self) -> crate::OperationError {
        match self {
            Self::Limit => crate::OperationError::ResultTooLarge,
            Self::Serialize(source) => crate::OperationError::Internal(Box::new(source)),
        }
    }
}

struct BoundedWriter {
    bytes: Vec<u8>,
    limit: usize,
    exceeded: bool,
}
impl Write for BoundedWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if bytes.len() > self.limit.saturating_sub(self.bytes.len()) {
            self.exceeded = true;
            return Err(io::Error::other("JSON size limit exceeded"));
        }
        self.bytes.extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
