use crate::{az_cli::TokenScope, PimClient};
use anyhow::{bail, Context, Result};
use futures::future::join_all;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
use tracing::info;

#[derive(Deserialize, Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone)]
pub struct Object {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
    pub object_type: PrincipalType,
}

#[derive(Deserialize, Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone)]
pub enum PrincipalType {
    User,
    Group,
    ServicePrincipal,
}

fn parse_objects(value: &Value) -> Result<BTreeSet<Object>> {
    let mut results = BTreeSet::new();
    if let Some(values) = value.get("value").and_then(|x| x.as_array()) {
        for value in values {
            let Some(id) = value
                .get("id")
                .map(|v| v.as_str().unwrap_or(""))
                .map(ToString::to_string)
            else {
                continue;
            };

            let Some(display_name) = value
                .get("displayName")
                .map(|v| v.as_str().unwrap_or(""))
                .map(ToString::to_string)
            else {
                continue;
            };

            let upn = value
                .get("userPrincipalName")
                .and_then(|v| v.as_str())
                .map(ToString::to_string);

            let data_type = value
                .get("@odata.type")
                .map(|x| x.as_str().unwrap_or(""))
                .context("missing @odata.type")?;

            let object_type = match data_type {
                "#microsoft.graph.user" => PrincipalType::User,
                "#microsoft.graph.group" => PrincipalType::Group,
                "#microsoft.graph.servicePrincipal" => PrincipalType::ServicePrincipal,
                _ => {
                    bail!("unknown object type: {data_type} - {value:#?}");
                }
            };
            results.insert(Object {
                id,
                display_name,
                upn,
                object_type,
            });
        }
    }

    Ok(results)
}

async fn get_objects_by_ids_small(
    pim_client: &PimClient,
    ids: &[&&str],
) -> Result<BTreeSet<Object>> {
    info!("checking {} objects", ids.len());
    let builder = pim_client
        .backend
        .client
        .request(
            Method::POST,
            "https://graph.microsoft.com/v1.0/directoryObjects/getByIds",
        )
        .bearer_auth(pim_client.backend.get_token(TokenScope::Graph).await?);

    let body = serde_json::json!({ "ids": ids });
    let request = builder.json(&body).build()?;
    let value = pim_client.backend.retry_request(&request, None).await?;

    parse_objects(&value)
}

pub(crate) async fn get_objects_by_ids(
    pim_client: &PimClient,
    ids: BTreeSet<&str>,
) -> Result<BTreeMap<String, Object>> {
    let mut cache = pim_client.object_cache.lock().await;
    let to_update = ids
        .iter()
        .filter(|id| !cache.contains_key(**id))
        .collect::<Vec<_>>();

    let chunks = to_update.chunks(50).collect::<Vec<_>>();

    let results = join_all(
        chunks
            .iter()
            .map(|chunk| get_objects_by_ids_small(pim_client, chunk)),
    )
    .await;

    for entry in results {
        for entry in entry? {
            cache.insert(entry.id.clone(), Some(entry));
        }
    }

    let mut result = BTreeMap::new();
    for id in ids {
        if let Some(entry) = cache.get(id).cloned() {
            if let Some(entry) = entry {
                result.insert(entry.id.clone(), entry);
            }
        } else {
            cache.insert(id.to_string(), None);
        }
    }

    Ok(result)
}

pub(crate) async fn group_members(pim_client: &PimClient, id: &str) -> Result<BTreeSet<Object>> {
    let mut group_cache = pim_client.group_cache.lock().await;
    if let Some(entries) = group_cache.get(id) {
        return Ok(entries.clone());
    }

    let mut cache = pim_client.object_cache.lock().await;

    let url = format!("https://graph.microsoft.com/v1.0/groups/{id}/members");
    let request = pim_client
        .backend
        .client
        .request(Method::GET, &url)
        .bearer_auth(pim_client.backend.get_token(TokenScope::Graph).await?)
        .build()?;
    let value = pim_client.backend.retry_request(&request, None).await?;
    let results = parse_objects(&value)?;

    for object in &results {
        if cache.get(&object.id).is_none() {
            cache.insert(object.id.clone(), Some(object.clone()));
        }
    }

    group_cache.insert(id.to_string(), results.clone());

    Ok(results)
}
