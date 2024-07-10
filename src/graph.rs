use crate::{az_cli::TokenScope, PimClient};
use anyhow::Result;
use parking_lot::Mutex;
use reqwest::Method;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};

static CACHE: Mutex<BTreeMap<String, Object>> = Mutex::new(BTreeMap::new());

#[derive(Debug, Serialize, Deserialize, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Object {
    pub id: String,
    pub display_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub upn: Option<String>,
}

fn get_objects_by_ids_small(pim_client: &PimClient, ids: &[&&str]) -> Result<Vec<Object>> {
    let builder = pim_client
        .backend
        .client
        .request(
            Method::POST,
            "https://graph.microsoft.com/v1.0/directoryObjects/getByIds",
        )
        .bearer_auth(pim_client.backend.get_token(TokenScope::Graph)?);

    let body = serde_json::json!({ "ids": ids });
    let request = builder.json(&body).build()?;
    let value = pim_client.backend.retry_request(&request, None)?;

    let mut results = vec![];
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

            results.push(Object {
                id,
                display_name,
                upn,
            });
        }
    }

    Ok(results)
}

pub(crate) fn get_objects_by_ids(
    pim_client: &PimClient,
    ids: BTreeSet<&str>,
) -> Result<BTreeMap<String, Object>> {
    let mut cache = CACHE.lock();
    let to_update = ids
        .iter()
        .filter(|id| !cache.contains_key(**id))
        .collect::<Vec<_>>();

    let mut result = BTreeMap::new();

    for chunk in to_update.chunks(50) {
        for entry in get_objects_by_ids_small(pim_client, chunk)? {
            cache.insert(entry.id.clone(), entry);
        }
    }

    for id in ids {
        if let Some(entry) = cache.get(id) {
            result.insert(entry.id.clone(), entry.clone());
        }
    }

    Ok(result)
}
