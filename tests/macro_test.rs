use mnemosyne_rs::{protect, Config, DynamoDbPersistence, Error, Mnemosyne, PollStrategy};
use std::sync::Arc;
use std::time::Duration;
use uuid::Uuid;

// Helper to create a test DynamoDB client
async fn create_test_client() -> aws_sdk_dynamodb::Client {
    let endpoint = std::env::var("MNEMOSYNE_DYNAMODB_ENDPOINT")
        .unwrap_or_else(|_| "http://localhost:8000".to_string());

    let config = aws_config::from_env().endpoint_url(endpoint).load().await;
    aws_sdk_dynamodb::Client::new(&config)
}

// Helper to create a test table
async fn create_test_table(client: &aws_sdk_dynamodb::Client, table_name: &str) {
    use aws_sdk_dynamodb::types::{
        AttributeDefinition, BillingMode, KeySchemaElement, KeyType, ScalarAttributeType,
    };

    let _ = client
        .create_table()
        .table_name(table_name)
        .billing_mode(BillingMode::PayPerRequest)
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("id")
                .key_type(KeyType::Hash)
                .build()
                .unwrap(),
        )
        .key_schema(
            KeySchemaElement::builder()
                .attribute_name("processorId")
                .key_type(KeyType::Range)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("id")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .attribute_definitions(
            AttributeDefinition::builder()
                .attribute_name("processorId")
                .attribute_type(ScalarAttributeType::S)
                .build()
                .unwrap(),
        )
        .send()
        .await;
}

// Helper to delete a test table
async fn delete_test_table(client: &aws_sdk_dynamodb::Client, table_name: &str) {
    let _ = client.delete_table().table_name(table_name).send().await;
}

// Example struct that uses the macro
struct EmailService {
    mnemosyne: Arc<Mnemosyne<Uuid, Uuid, String>>,
}

#[derive(Debug, Clone)]
struct Email {
    id: Uuid,
    recipient: String,
    subject: String,
    body: String,
}

impl EmailService {
    fn new(mnemosyne: Arc<Mnemosyne<Uuid, Uuid, String>>) -> Self {
        Self { mnemosyne }
    }

    /// Send an email with deduplication using the macro
    #[protect(mnemosyne = self.mnemosyne.clone(), id = email.id)]
    async fn send_email(&self, email: Email) -> Result<String, Error> {
        // Simulate sending email
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(format!(
            "Email sent to {} with subject: {}",
            email.recipient, email.subject
        ))
    }

    /// Another example with a function call to compute the id
    #[protect(mnemosyne = self.mnemosyne.clone(), id = Self::compute_email_id(&email))]
    async fn send_email_with_computed_id(&self, email: Email) -> Result<String, Error> {
        // Simulate sending email
        tokio::time::sleep(Duration::from_millis(10)).await;
        Ok(format!(
            "Email sent to {} with subject: {}",
            email.recipient, email.subject
        ))
    }

    /// Helper function to compute a deterministic ID from email recipient and subject
    fn compute_email_id(email: &Email) -> Uuid {
        // Create a simple deterministic UUID based on recipient and subject
        // In real code you might use a proper hash or UUID v5
        let data = format!("{}-{}", email.recipient, email.subject);
        let hash = data.bytes().fold(0u128, |acc, b| acc.wrapping_mul(31).wrapping_add(b as u128));
        Uuid::from_u128(hash)
    }
}

#[tokio::test]
async fn test_protect_macro_basic() {
    let client = create_test_client().await;
    let table_name = format!("test-macro-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne = Arc::new(Mnemosyne::new(persistence, config));
    let service = EmailService::new(mnemosyne);

    let email = Email {
        id: Uuid::new_v4(),
        recipient: "test@example.com".to_string(),
        subject: "Test Subject".to_string(),
        body: "Test Body".to_string(),
    };

    // First call should execute
    let result1 = service.send_email(email.clone()).await.unwrap();
    assert!(result1.contains("Email sent to test@example.com"));

    // Second call with same email.id should return memoized result
    let result2 = service.send_email(email.clone()).await.unwrap();
    assert_eq!(result1, result2);

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_protect_macro_with_computed_id() {
    let client = create_test_client().await;
    let table_name = format!("test-macro-computed-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne = Arc::new(Mnemosyne::new(persistence, config));
    let service = EmailService::new(mnemosyne);

    let email = Email {
        id: Uuid::new_v4(), // Different UUID each time, but computed ID will be the same
        recipient: "test@example.com".to_string(),
        subject: "Test Subject".to_string(),
        body: "Test Body".to_string(),
    };

    // First call should execute
    let result1 = service.send_email_with_computed_id(email.clone()).await.unwrap();
    assert!(result1.contains("Email sent to test@example.com"));

    // Second call with different email.id but same recipient+subject should return memoized result
    let email2 = Email {
        id: Uuid::new_v4(), // Different UUID!
        ..email.clone()
    };
    let result2 = service.send_email_with_computed_id(email2).await.unwrap();
    assert_eq!(result1, result2);

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_protect_macro_different_ids() {
    let client = create_test_client().await;
    let table_name = format!("test-macro-diff-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne = Arc::new(Mnemosyne::new(persistence, config));
    let service = EmailService::new(mnemosyne);

    let email1 = Email {
        id: Uuid::new_v4(),
        recipient: "test1@example.com".to_string(),
        subject: "Test Subject 1".to_string(),
        body: "Test Body 1".to_string(),
    };

    let email2 = Email {
        id: Uuid::new_v4(),
        recipient: "test2@example.com".to_string(),
        subject: "Test Subject 2".to_string(),
        body: "Test Body 2".to_string(),
    };

    // Both should execute successfully as they have different IDs
    let result1 = service.send_email(email1).await.unwrap();
    let result2 = service.send_email(email2).await.unwrap();

    assert!(result1.contains("test1@example.com"));
    assert!(result2.contains("test2@example.com"));
    assert_ne!(result1, result2);

    delete_test_table(&client, &table_name).await;
}
