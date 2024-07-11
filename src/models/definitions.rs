use serde::{Deserialize, Serialize};

#[derive(Deserialize, Debug, Serialize)]
pub(crate) struct Definitions {
    pub(crate) value: Vec<Definition>,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Definition {
    pub id: String,
    pub name: String,
    pub properties: Properties,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct Properties {
    pub assignable_scopes: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_by: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_on: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub updated_by: Option<String>,
    pub description: String,
    pub permissions: Vec<Permission>,
    pub role_name: String,
    #[serde(rename = "type")]
    pub type_: String,
}

#[derive(Deserialize, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
#[serde(deny_unknown_fields)]
pub struct Permission {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub actions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_actions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data_actions: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub not_data_actions: Option<Vec<String>>,
}

#[cfg(test)]
mod tests {
    use super::Definitions;
    use anyhow::Result;
    use insta::assert_json_snapshot;

    #[test]
    fn test_deserialization() -> Result<()> {
        const ROLES: &str = include_str!("../../tests/data/definitions.json");
        let definitions: Definitions = serde_json::from_str(ROLES)?;
        assert_json_snapshot!(definitions);
        Ok(())
    }
}
