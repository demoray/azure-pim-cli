use crate::{graph::Object, scope::Scope};
use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Serialize)]
pub(crate) struct Assignments {
    pub(crate) value: Vec<Assignment>,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Assignment {
    pub id: String,
    pub name: String,
    pub properties: Properties,
    #[serde(rename = "type")]
    pub type_: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub object: Option<Object>,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct Properties {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub condition_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub role_definition_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delegated_managed_identity_resource_id: Option<String>,
    pub principal_id: String,
    pub principal_type: String,
    pub scope: Scope,
}

#[cfg(test)]
mod tests {
    use super::Assignments;
    use anyhow::Result;
    use insta::assert_json_snapshot;

    #[test]
    fn test_deserialization() -> Result<()> {
        const DATA: &str = include_str!("../tests/data/assignments.json");
        let data: Assignments = serde_json::from_str(DATA)?;
        assert_json_snapshot!(data);
        Ok(())
    }
}
