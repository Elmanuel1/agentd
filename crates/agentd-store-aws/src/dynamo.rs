//! DynamoDB document helper — a thin, uniform (pk, sk, doc-JSON) store that
//! every entity maps onto. Keeps AWS-SDK noise out of the entity impls.

use std::collections::HashMap;

use aws_sdk_dynamodb::error::SdkError;
use aws_sdk_dynamodb::operation::put_item::PutItemError;
use aws_sdk_dynamodb::types::{
    AttributeDefinition, AttributeValue, BillingMode, KeySchemaElement, KeyType,
    ScalarAttributeType,
};
use aws_sdk_dynamodb::Client;
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::AwsError;

/// Uniform DynamoDB document access.
#[derive(Clone, Debug)]
pub struct DynamoDoc {
    client: Client,
}

impl DynamoDoc {
    /// Wrap a client.
    pub fn new(client: Client) -> Self {
        Self { client }
    }

    /// Create the `(pk, sk)` table if it doesn't exist (idempotent).
    pub async fn ensure_table(&self, table: &str) -> Result<(), AwsError> {
        let existing = self
            .client
            .list_tables()
            .send()
            .await
            .map_err(|e| AwsError::Dynamo(e.to_string()))?;
        if existing
            .table_names()
            .iter()
            .any(|t| t == table)
        {
            return Ok(());
        }
        let s = |n: &str| {
            AttributeDefinition::builder()
                .attribute_name(n)
                .attribute_type(ScalarAttributeType::S)
                .build()
        };
        let key = |n: &str, t: KeyType| KeySchemaElement::builder().attribute_name(n).key_type(t).build();
        self.client
            .create_table()
            .table_name(table)
            .billing_mode(BillingMode::PayPerRequest)
            .attribute_definitions(s("pk").map_err(|e| AwsError::Dynamo(e.to_string()))?)
            .attribute_definitions(s("sk").map_err(|e| AwsError::Dynamo(e.to_string()))?)
            .key_schema(key("pk", KeyType::Hash).map_err(|e| AwsError::Dynamo(e.to_string()))?)
            .key_schema(key("sk", KeyType::Range).map_err(|e| AwsError::Dynamo(e.to_string()))?)
            .send()
            .await
            .map_err(|e| AwsError::Dynamo(e.to_string()))?;
        Ok(())
    }

    /// Upsert a document.
    pub async fn put<T: Serialize>(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
        doc: &T,
    ) -> Result<(), AwsError> {
        let json = serde_json::to_string(doc).map_err(|e| AwsError::Decode(e.to_string()))?;
        self.client
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(pk.to_owned()))
            .item("sk", AttributeValue::S(sk.to_owned()))
            .item("doc", AttributeValue::S(json))
            .send()
            .await
            .map_err(|e| AwsError::Dynamo(e.to_string()))?;
        Ok(())
    }

    /// Insert only if `(pk, sk)` is absent. Returns `true` if this call
    /// created the item, `false` if it already existed — the **atomic
    /// cross-replica claim** (exactly-once).
    pub async fn put_if_absent(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
    ) -> Result<bool, AwsError> {
        let res = self
            .client
            .put_item()
            .table_name(table)
            .item("pk", AttributeValue::S(pk.to_owned()))
            .item("sk", AttributeValue::S(sk.to_owned()))
            .condition_expression("attribute_not_exists(pk)")
            .send()
            .await;
        match res {
            Ok(_) => Ok(true),
            Err(SdkError::ServiceError(se))
                if matches!(se.err(), PutItemError::ConditionalCheckFailedException(_)) =>
            {
                Ok(false)
            }
            Err(e) => Err(AwsError::Dynamo(e.to_string())),
        }
    }

    /// Get one document.
    pub async fn get<T: DeserializeOwned>(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
    ) -> Result<Option<T>, AwsError> {
        let out = self
            .client
            .get_item()
            .table_name(table)
            .key("pk", AttributeValue::S(pk.to_owned()))
            .key("sk", AttributeValue::S(sk.to_owned()))
            .send()
            .await
            .map_err(|e| AwsError::Dynamo(e.to_string()))?;
        match out.item().and_then(|i| i.get("doc")) {
            Some(AttributeValue::S(json)) => {
                let v = serde_json::from_str(json).map_err(|e| AwsError::Decode(e.to_string()))?;
                Ok(Some(v))
            }
            _ => Ok(None),
        }
    }

    /// Query all documents under a partition, ascending by `sk`. `sk_prefix`
    /// optionally restricts to a `begins_with` range.
    pub async fn query<T: DeserializeOwned>(
        &self,
        table: &str,
        pk: &str,
        sk_prefix: Option<&str>,
    ) -> Result<Vec<T>, AwsError> {
        let mut expr = "pk = :pk".to_owned();
        let mut vals: HashMap<String, AttributeValue> = HashMap::new();
        vals.insert(":pk".into(), AttributeValue::S(pk.to_owned()));
        if let Some(prefix) = sk_prefix {
            expr.push_str(" AND begins_with(sk, :skp)");
            vals.insert(":skp".into(), AttributeValue::S(prefix.to_owned()));
        }
        let mut out = Vec::new();
        let mut last: Option<HashMap<String, AttributeValue>> = None;
        loop {
            let mut req = self
                .client
                .query()
                .table_name(table)
                .key_condition_expression(&expr)
                .set_expression_attribute_values(Some(vals.clone()));
            if let Some(lek) = &last {
                req = req.set_exclusive_start_key(Some(lek.clone()));
            }
            let page = req.send().await.map_err(|e| AwsError::Dynamo(e.to_string()))?;
            for item in page.items() {
                if let Some(AttributeValue::S(json)) = item.get("doc") {
                    out.push(
                        serde_json::from_str(json).map_err(|e| AwsError::Decode(e.to_string()))?,
                    );
                }
            }
            match page.last_evaluated_key() {
                Some(k) if !k.is_empty() => last = Some(k.clone()),
                _ => break,
            }
        }
        Ok(out)
    }

    /// Delete one document; returns the old value if present.
    pub async fn delete<T: DeserializeOwned>(
        &self,
        table: &str,
        pk: &str,
        sk: &str,
    ) -> Result<Option<T>, AwsError> {
        let out = self
            .client
            .delete_item()
            .table_name(table)
            .key("pk", AttributeValue::S(pk.to_owned()))
            .key("sk", AttributeValue::S(sk.to_owned()))
            .return_values(aws_sdk_dynamodb::types::ReturnValue::AllOld)
            .send()
            .await
            .map_err(|e| AwsError::Dynamo(e.to_string()))?;
        match out.attributes().and_then(|a| a.get("doc")) {
            Some(AttributeValue::S(json)) => Ok(Some(
                serde_json::from_str(json).map_err(|e| AwsError::Decode(e.to_string()))?,
            )),
            _ => Ok(None),
        }
    }
}
