use mnemosyne_rs::{Config, DynamoDbPersistence, Mnemosyne, Outcome, PollStrategy};
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

#[tokio::test]
async fn test_new_process() {
    tracing_subscriber::fmt::init();

    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence, config);

    let signal_id = Uuid::new_v4();
    let result = mnemosyne.try_start_process(signal_id).await.unwrap();

    match result {
        Outcome::New { complete_process } => {
            complete_process("test-result".to_string()).await.unwrap();
        }
        Outcome::Duplicate { .. } => panic!("Expected New, got Duplicate"),
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_duplicate_process() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence, config);

    let signal_id = Uuid::new_v4();

    // First attempt - should be New
    let result1 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match result1 {
        Outcome::New { complete_process } => {
            complete_process("test-result".to_string()).await.unwrap();
        }
        Outcome::Duplicate { .. } => panic!("Expected New on first attempt"),
    }

    // Second attempt - should be Duplicate
    let result2 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match result2 {
        Outcome::New { .. } => panic!("Expected Duplicate on second attempt"),
        Outcome::Duplicate { value } => {
            assert_eq!(value, "test-result");
        }
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_protect() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence, config);

    let signal_id = Uuid::new_v4();
    let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // First call - should execute
    let counter_clone = Arc::clone(&counter);
    let result1 = mnemosyne
        .protect(signal_id, || async move {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("computed-result".to_string())
        })
        .await
        .unwrap();

    assert_eq!(result1, "computed-result");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Second call - should return memoized result without executing
    let counter_clone = Arc::clone(&counter);
    let result2 = mnemosyne
        .protect(signal_id, || async move {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok("should-not-execute".to_string())
        })
        .await
        .unwrap();

    assert_eq!(result2, "computed-result");
    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1); // Should still be 1

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_concurrent_processing() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::backoff(Duration::from_millis(50), 1.5, Duration::from_secs(10)),
    );

    let mnemosyne = Arc::new(Mnemosyne::<Uuid, Uuid, String>::new(persistence, config));
    let signal_id = Uuid::new_v4();
    let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Launch 50 concurrent requests for the same signal
    let mut handles = vec![];
    for i in 0..50 {
        let mnemosyne_clone = Arc::clone(&mnemosyne);
        let counter_clone = Arc::clone(&counter);

        let handle = tokio::spawn(async move {
            mnemosyne_clone
                .protect(signal_id, || async move {
                    counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(100)).await;
                    Ok(format!("result-{}", i))
                })
                .await
        });

        handles.push(handle);
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All should succeed
    for result in results.iter() {
        assert!(result.is_ok());
        assert!(result.as_ref().unwrap().is_ok());
    }

    // Counter should be 1 (only executed once)
    let final_count = counter.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(final_count, 1, "Process should only execute once");

    // All results should be the same
    let first_result = results[0].as_ref().unwrap().as_ref().unwrap();
    for result in results.iter() {
        let value = result.as_ref().unwrap().as_ref().unwrap();
        assert_eq!(value, first_result);
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_invalidate() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    let mnemosyne: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence, config);

    let signal_id = Uuid::new_v4();

    // Process signal
    let result = mnemosyne
        .protect(signal_id, || async { Ok("test-result".to_string()) })
        .await
        .unwrap();
    assert_eq!(result, "test-result");

    // Verify it returns duplicate
    let result2 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match result2 {
        Outcome::Duplicate { .. } => {}
        _ => panic!("Expected Duplicate"),
    }

    // Invalidate
    mnemosyne.invalidate(signal_id).await.unwrap();

    // Should be new again
    let result3 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match result3 {
        Outcome::New { .. } => {}
        _ => panic!("Expected New after invalidation"),
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_deduplication_without_memoization() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(100), Duration::from_secs(10)),
    );

    // Use () as the result type - no memoization needed
    let mnemosyne: Mnemosyne<Uuid, Uuid, ()> = Mnemosyne::new(persistence, config);

    let signal_id = Uuid::new_v4();
    let counter = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // First call - should execute
    let counter_clone = Arc::clone(&counter);
    mnemosyne
        .protect(signal_id, || async move {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(()) // Return unit type
        })
        .await
        .unwrap();

    assert_eq!(counter.load(std::sync::atomic::Ordering::SeqCst), 1);

    // Second call - should NOT execute (deduplication works)
    let counter_clone = Arc::clone(&counter);
    mnemosyne
        .protect(signal_id, || async move {
            counter_clone.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(())
        })
        .await
        .unwrap();

    // Counter should still be 1 - second call was deduplicated
    assert_eq!(
        counter.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "Process should only execute once even without memoization"
    );

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_unit_type_with_concurrent_requests() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::backoff(Duration::from_millis(50), 1.5, Duration::from_secs(10)),
    );

    let mnemosyne = Arc::new(Mnemosyne::<Uuid, Uuid, ()>::new(persistence, config));
    let signal_id = Uuid::new_v4();
    let execution_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Launch 20 concurrent requests with unit type
    let mut handles = vec![];
    for _ in 0..20 {
        let mnemosyne_clone = Arc::clone(&mnemosyne);
        let exec_count = Arc::clone(&execution_count);

        let handle = tokio::spawn(async move {
            mnemosyne_clone
                .protect(signal_id, || async move {
                    exec_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    tokio::time::sleep(Duration::from_millis(50)).await;
                    Ok(())
                })
                .await
        });

        handles.push(handle);
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All should succeed
    for result in results.iter() {
        assert!(result.is_ok());
        assert!(result.as_ref().unwrap().is_ok());
    }

    // Should only execute once
    let executions = execution_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        executions, 1,
        "Process with unit type should only execute once across {} concurrent requests",
        results.len()
    );

    delete_test_table(&client, &table_name).await;
}
