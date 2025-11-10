use thiserror::Error;

#[derive(Error, Debug)]
pub enum Error {
    #[error("DynamoDB error: {0}")]
    DynamoDb(String),

    #[error("Encoding error: {0}")]
    Encoding(String),

    #[error("Decoding error: {0}")]
    Decoding(String),

    #[error("Process timeout")]
    Timeout,

    #[error("Process expired")]
    Expired,

    #[error("Internal error: {0}")]
    Internal(String),
}

impl From<aws_sdk_dynamodb::Error> for Error {
    fn from(err: aws_sdk_dynamodb::Error) -> Self {
        Error::DynamoDb(err.to_string())
    }
}

impl<E> From<aws_sdk_dynamodb::error::SdkError<E>> for Error
where
    E: std::error::Error + Send + Sync + 'static,
{
    fn from(err: aws_sdk_dynamodb::error::SdkError<E>) -> Self {
        Error::DynamoDb(err.to_string())
    }
}

impl From<serde_json::Error> for Error {
    fn from(err: serde_json::Error) -> Self {
        Error::Encoding(err.to_string())
    }
}
