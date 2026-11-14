/// Whether a failed statement may succeed when retried: a lost or exhausted connection, a
/// serialization or lock conflict, a cancelled statement or a server shutting down. The same
/// classification as `product-listing-postgres`.
pub(crate) fn is_temporary(error: &sqlx::Error) -> bool {
    match error {
        sqlx::Error::Io(_)
        | sqlx::Error::Tls(_)
        | sqlx::Error::PoolTimedOut
        | sqlx::Error::PoolClosed
        | sqlx::Error::WorkerCrashed => true,
        sqlx::Error::Database(error) => error.code().is_some_and(|code| {
            code.starts_with("08")
                || code.starts_with("53")
                || matches!(
                    code.as_ref(),
                    "40001" | "40P01" | "55P03" | "57014" | "57P01" | "57P02" | "57P03"
                )
        }),
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::is_temporary;

    #[test]
    fn should_treat_connection_failures_as_temporary_and_decode_failures_as_permanent() {
        assert!(is_temporary(&sqlx::Error::PoolTimedOut));
        assert!(is_temporary(&sqlx::Error::Io(std::io::Error::from(
            std::io::ErrorKind::ConnectionReset
        ))));
        assert!(!is_temporary(&sqlx::Error::RowNotFound));
        assert!(!is_temporary(&sqlx::Error::ColumnNotFound("x".into())));
    }
}
