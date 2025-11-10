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
async fn test_exactly_once_with_manual_control() {
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
    let execution_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let completion_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Launch 100 concurrent attempts using try_start_process
    let mut handles = vec![];
    for _ in 0..100 {
        let mnemosyne_clone = Arc::clone(&mnemosyne);
        let exec_count = Arc::clone(&execution_count);
        let comp_count = Arc::clone(&completion_count);

        let handle = tokio::spawn(async move {
            match mnemosyne_clone.try_start_process(signal_id).await.unwrap() {
                Outcome::New { complete_process } => {
                    // Only one process should get here
                    exec_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);

                    // Simulate some work
                    tokio::time::sleep(Duration::from_millis(50)).await;

                    // Complete the process
                    complete_process(format!(
                        "result-{}",
                        exec_count.load(std::sync::atomic::Ordering::SeqCst)
                    ))
                    .await
                    .unwrap();

                    comp_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    true
                }
                Outcome::Duplicate { .. } => {
                    // All others should get here
                    false
                }
            }
        });

        handles.push(handle);
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // Exactly one should have executed
    let executed_count = results.iter().filter(|r| *r.as_ref().unwrap()).count();
    assert_eq!(
        executed_count, 1,
        "Exactly one process should have executed, but {} did",
        executed_count
    );

    // Verify counters
    assert_eq!(
        execution_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "Execution count should be 1"
    );
    assert_eq!(
        completion_count.load(std::sync::atomic::Ordering::SeqCst),
        1,
        "Completion count should be 1"
    );

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_concurrent_different_signals() {
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
    let execution_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Process 10 different signals, each with 10 concurrent attempts
    let mut handles = vec![];
    for signal_num in 0..10 {
        let signal_id = Uuid::new_v4();

        for _ in 0..10 {
            let mnemosyne_clone = Arc::clone(&mnemosyne);
            let exec_count = Arc::clone(&execution_count);

            let handle = tokio::spawn(async move {
                mnemosyne_clone
                    .protect(signal_id, || async move {
                        exec_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(10)).await;
                        Ok(format!("result-{}", signal_num))
                    })
                    .await
                    .unwrap()
            });

            handles.push(handle);
        }
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All should succeed
    assert_eq!(results.len(), 100);
    for result in results.iter() {
        assert!(result.is_ok());
    }

    // Each signal should execute exactly once, so total executions = 10
    let total_executions = execution_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        total_executions, 10,
        "Each of 10 signals should execute exactly once, but got {} total executions",
        total_executions
    );

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_timeout_recovery() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    // Short timeout for testing
    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_millis(500), // 500ms timeout
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(50), Duration::from_secs(1)),
    );

    let mnemosyne: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence, config);
    let signal_id = Uuid::new_v4();

    // Start processing but don't complete
    let outcome1 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match outcome1 {
        Outcome::New { .. } => {
            // Intentionally NOT completing the process
            // This simulates a stuck/failed process
        }
        Outcome::Duplicate { .. } => panic!("First attempt should be New"),
    }

    // Immediately try again - should see it as Running
    let outcome2 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match outcome2 {
        Outcome::New { .. } => {
            // Due to polling, it might see timeout and allow retry
            // This is acceptable
        }
        Outcome::Duplicate { .. } => panic!("Should not be Duplicate yet"),
    }

    // Wait for timeout
    tokio::time::sleep(Duration::from_millis(600)).await;

    // Now it should allow retry due to timeout
    let outcome3 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match outcome3 {
        Outcome::New { complete_process } => {
            // Complete successfully this time
            complete_process("recovered-result".to_string())
                .await
                .unwrap();
        }
        Outcome::Duplicate { .. } => panic!("Should allow retry after timeout"),
    }

    // Verify it's now completed
    let outcome4 = mnemosyne.try_start_process(signal_id).await.unwrap();
    match outcome4 {
        Outcome::New { .. } => panic!("Should be Duplicate after successful completion"),
        Outcome::Duplicate { value } => {
            assert_eq!(value, "recovered-result");
        }
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_multiple_processors() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    // Create two different "processor instances"
    let processor_id_1 = Uuid::new_v4();
    let processor_id_2 = Uuid::new_v4();

    let config1 = Config::new(
        processor_id_1,
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(50), Duration::from_secs(5)),
    );

    let config2 = Config::new(
        processor_id_2,
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::linear(Duration::from_millis(50), Duration::from_secs(5)),
    );

    let persistence1: Arc<dyn mnemosyne_rs::Persistence<Uuid, Uuid, String>> =
        Arc::clone(&persistence) as Arc<dyn mnemosyne_rs::Persistence<Uuid, Uuid, String>>;
    let persistence2: Arc<dyn mnemosyne_rs::Persistence<Uuid, Uuid, String>> =
        Arc::clone(&persistence) as Arc<dyn mnemosyne_rs::Persistence<Uuid, Uuid, String>>;

    let mnemosyne1: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence1, config1);
    let mnemosyne2: Mnemosyne<Uuid, Uuid, String> = Mnemosyne::new(persistence2, config2);

    let signal_id = Uuid::new_v4();

    // Each processor should be able to process the same signal independently
    let result1 = mnemosyne1
        .protect(signal_id, || async { Ok("processor-1-result".to_string()) })
        .await
        .unwrap();

    let result2 = mnemosyne2
        .protect(signal_id, || async { Ok("processor-2-result".to_string()) })
        .await
        .unwrap();

    assert_eq!(result1, "processor-1-result");
    assert_eq!(result2, "processor-2-result");

    // Each processor should see its own result as duplicate
    let dup1 = mnemosyne1.try_start_process(signal_id).await.unwrap();
    match dup1 {
        Outcome::Duplicate { value } => assert_eq!(value, "processor-1-result"),
        _ => panic!("Processor 1 should see duplicate"),
    }

    let dup2 = mnemosyne2.try_start_process(signal_id).await.unwrap();
    match dup2 {
        Outcome::Duplicate { value } => assert_eq!(value, "processor-2-result"),
        _ => panic!("Processor 2 should see duplicate"),
    }

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_high_concurrency_stress() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::backoff(Duration::from_millis(10), 1.5, Duration::from_secs(20)),
    );

    let mnemosyne = Arc::new(Mnemosyne::<Uuid, Uuid, String>::new(persistence, config));
    let signal_id = Uuid::new_v4();
    let execution_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
    let success_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Launch 200 concurrent requests
    let mut handles = vec![];
    for i in 0..200 {
        let mnemosyne_clone = Arc::clone(&mnemosyne);
        let exec_count = Arc::clone(&execution_count);
        let succ_count = Arc::clone(&success_count);

        let handle = tokio::spawn(async move {
            match mnemosyne_clone
                .protect(signal_id, || async move {
                    exec_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    // Simulate varying work durations
                    tokio::time::sleep(Duration::from_millis(10 + (i % 50))).await;
                    Ok(format!("result-{}", i))
                })
                .await
            {
                Ok(_) => {
                    succ_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    true
                }
                Err(_) => false,
            }
        });

        handles.push(handle);
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All should complete successfully
    let successful = results.iter().filter(|r| *r.as_ref().unwrap()).count();
    assert_eq!(
        successful, 200,
        "All 200 requests should succeed, but only {} did",
        successful
    );

    // Only one should have executed
    let executions = execution_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        executions, 1,
        "Process should execute exactly once, but executed {} times",
        executions
    );

    // All should have received a result
    let successes = success_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(successes, 200, "All requests should get result");

    delete_test_table(&client, &table_name).await;
}

#[tokio::test]
async fn test_staggered_concurrent_requests() {
    let client = create_test_client().await;
    let table_name = format!("test-mnemosyne-{}", Uuid::new_v4());

    create_test_table(&client, &table_name).await;

    let persistence = Arc::new(DynamoDbPersistence::new(client.clone(), table_name.clone()));

    let config = Config::new(
        Uuid::new_v4(),
        Duration::from_secs(60),
        Some(Duration::from_secs(3600)),
        PollStrategy::backoff(Duration::from_millis(20), 2.0, Duration::from_secs(15)),
    );

    let mnemosyne = Arc::new(Mnemosyne::<Uuid, Uuid, String>::new(persistence, config));
    let signal_id = Uuid::new_v4();
    let execution_count = Arc::new(std::sync::atomic::AtomicU32::new(0));

    // Launch requests in waves with delays
    let mut handles = vec![];
    for wave in 0..5 {
        // Each wave launches 20 concurrent requests
        for i in 0..20 {
            let mnemosyne_clone = Arc::clone(&mnemosyne);
            let exec_count = Arc::clone(&execution_count);

            let handle = tokio::spawn(async move {
                // Stagger the requests slightly within each wave
                tokio::time::sleep(Duration::from_millis(i * 5)).await;

                mnemosyne_clone
                    .protect(signal_id, || async move {
                        exec_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                        tokio::time::sleep(Duration::from_millis(100)).await;
                        Ok(format!("result-wave-{}", wave))
                    })
                    .await
                    .unwrap()
            });

            handles.push(handle);
        }

        // Small delay between waves
        tokio::time::sleep(Duration::from_millis(50)).await;
    }

    // Wait for all to complete
    let results: Vec<_> = futures::future::join_all(handles).await;

    // All should succeed
    assert_eq!(results.len(), 100);
    for result in results.iter() {
        assert!(result.is_ok());
    }

    // Only one execution despite staggered requests
    let executions = execution_count.load(std::sync::atomic::Ordering::SeqCst);
    assert_eq!(
        executions, 1,
        "Process should execute exactly once even with staggered requests, but executed {} times",
        executions
    );

    delete_test_table(&client, &table_name).await;
}
