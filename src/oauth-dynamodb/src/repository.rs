use aws_sdk_dynamodb::Client;

#[derive(Debug, Clone)]
pub struct OAuthDynamoDbStore<'a> {
    client: &'a Client,
    table: String,
}

impl<'a> OAuthDynamoDbStore<'a> {
    pub fn new(client: &'a Client, table: impl Into<String>) -> Self {
        Self {
            client,
            table: table.into(),
        }
    }

    pub(crate) fn client(&self) -> &Client {
        self.client
    }
    pub(crate) fn table(&self) -> &str {
        &self.table
    }
}

pub type OAuthDynamoDbRepositoryImpl<'a> = OAuthDynamoDbStore<'a>;
