use crate::error::Error;
use crate::model::{Expiration, Process};
use crate::persistence::Persistence;
use async_trait::async_trait;
use aws_sdk_dynamodb::types::AttributeValue;
use aws_sdk_dynamodb::Client;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// DynamoDB-backed persistence implementation
pub struct DynamoDbPersistence {
    client: Client,
    table_name: String,
}

impl DynamoDbPersistence {
    pub fn new(client: Client, table_name: String) -> Self {
        Self { client, table_name }
    }
}

#[async_trait]
impl<Id, ProcessorId, A> Persistence<Id, ProcessorId, A> for DynamoDbPersistence
where
    Id: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
    ProcessorId: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
    A: Serialize + DeserializeOwned + Send + Sync + Clone + 'static,
{
    async fn start_processing_update(
        &self,
        id: Id,
        processor_id: ProcessorId,
        now: SystemTime,
    ) -> Result<Option<Process<Id, ProcessorId, A>>, Error> {
        let id_str = serde_json::to_string(&id)?;
        let processor_id_str = serde_json::to_string(&processor_id)?;
        let now_millis = now
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Internal(e.to_string()))?
            .as_millis() as i64;

        let result = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(id_str.clone()))
            .key("processorId", AttributeValue::S(processor_id_str.clone()))
            .update_expression("SET startedAt = if_not_exists(startedAt, :value)")
            .expression_attribute_values(":value", AttributeValue::N(now_millis.to_string()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllOld)
            .send()
            .await
            .map_err(|e| Error::DynamoDb(e.to_string()))?;

        if let Some(attributes) = result.attributes {
            if !attributes.is_empty() {
                return Ok(Some(decode_process(attributes)?));
            }
        }

        Ok(None)
    }

    async fn complete_process(
        &self,
        id: Id,
        processor_id: ProcessorId,
        now: SystemTime,
        ttl: Option<Duration>,
        value: A,
    ) -> Result<(), Error> {
        let id_str = serde_json::to_string(&id)?;
        let processor_id_str = serde_json::to_string(&processor_id)?;
        let now_millis = now
            .duration_since(UNIX_EPOCH)
            .map_err(|e| Error::Internal(e.to_string()))?
            .as_millis() as i64;

        let memoized_str = serde_json::to_string(&value)?;

        let mut update_builder = self
            .client
            .update_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(id_str))
            .key("processorId", AttributeValue::S(processor_id_str))
            .expression_attribute_values(":completedAt", AttributeValue::N(now_millis.to_string()))
            .expression_attribute_values(":memoized", AttributeValue::S(memoized_str));

        let update_expr = if let Some(ttl_duration) = ttl {
            let expires_on = (now + ttl_duration)
                .duration_since(UNIX_EPOCH)
                .map_err(|e| Error::Internal(e.to_string()))?
                .as_secs() as i64;

            update_builder = update_builder.expression_attribute_values(
                ":expiresOn",
                AttributeValue::N(expires_on.to_string()),
            );

            "SET completedAt = :completedAt, memoized = :memoized, expiresOn = :expiresOn"
        } else {
            "SET completedAt = :completedAt, memoized = :memoized"
        };

        update_builder
            .update_expression(update_expr)
            .send()
            .await
            .map_err(|e| Error::DynamoDb(e.to_string()))?;

        Ok(())
    }

    async fn invalidate_process(&self, id: Id, processor_id: ProcessorId) -> Result<(), Error> {
        let id_str = serde_json::to_string(&id)?;
        let processor_id_str = serde_json::to_string(&processor_id)?;

        self.client
            .delete_item()
            .table_name(&self.table_name)
            .key("id", AttributeValue::S(id_str))
            .key("processorId", AttributeValue::S(processor_id_str))
            .send()
            .await
            .map_err(|e| Error::DynamoDb(e.to_string()))?;

        Ok(())
    }
}

/// Decode a DynamoDB item into a Process
fn decode_process<Id, ProcessorId, A>(
    mut attributes: HashMap<String, AttributeValue>,
) -> Result<Process<Id, ProcessorId, A>, Error>
where
    Id: DeserializeOwned,
    ProcessorId: DeserializeOwned,
    A: DeserializeOwned,
{
    let id = attributes
        .remove("id")
        .and_then(|v| v.as_s().ok().cloned())
        .ok_or_else(|| Error::Decoding("Missing 'id' field".to_string()))?;
    let id: Id = serde_json::from_str(&id)?;

    let processor_id = attributes
        .remove("processorId")
        .and_then(|v| v.as_s().ok().cloned())
        .ok_or_else(|| Error::Decoding("Missing 'processorId' field".to_string()))?;
    let processor_id: ProcessorId = serde_json::from_str(&processor_id)?;

    let started_at = attributes
        .remove("startedAt")
        .and_then(|v| v.as_n().ok().and_then(|s| s.parse::<i64>().ok()))
        .ok_or_else(|| Error::Decoding("Missing or invalid 'startedAt' field".to_string()))?;
    let started_at = UNIX_EPOCH + Duration::from_millis(started_at as u64);

    let completed_at = attributes
        .remove("completedAt")
        .and_then(|v| v.as_n().ok().and_then(|s| s.parse::<i64>().ok()))
        .map(|millis| UNIX_EPOCH + Duration::from_millis(millis as u64));

    let expires_on = attributes
        .remove("expiresOn")
        .and_then(|v| v.as_n().ok().and_then(|s| s.parse::<i64>().ok()))
        .map(|secs| Expiration::new(UNIX_EPOCH + Duration::from_secs(secs as u64)));

    let memoized = attributes
        .remove("memoized")
        .and_then(|v| v.as_s().ok().cloned())
        .map(|s| serde_json::from_str(&s))
        .transpose()?;

    Ok(Process {
        id,
        processor_id,
        started_at,
        completed_at,
        expires_on,
        memoized,
    })
}
