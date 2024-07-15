use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::{
    fmt::{Display, Formatter, Result as FmtResult},
    str::FromStr,
};
use uuid::Uuid;

#[derive(thiserror::Error, Debug)]
pub enum ScopeError {
    #[error("scope must start with a /")]
    LeadingSlash,
}

#[derive(Serialize, PartialOrd, Ord, PartialEq, Eq, Debug, Clone, Deserialize, Hash)]
pub struct Scope(pub(crate) String);
impl Scope {
    pub fn new<S: Into<String>>(value: S) -> Result<Self, ScopeError> {
        let value = value.into();
        if !value.starts_with('/') {
            return Err(ScopeError::LeadingSlash);
        }
        Ok(Self(value))
    }

    #[must_use]
    pub fn from_subscription(subscription_id: &Uuid) -> Self {
        Self(format!("/subscriptions/{subscription_id}"))
    }

    #[must_use]
    pub fn from_resource_group(subscription_id: &Uuid, resource_group: &str) -> Self {
        Self(format!(
            "/subscriptions/{subscription_id}/resourceGroups/{resource_group}"
        ))
    }

    #[must_use]
    pub fn from_provider(subscription_id: &Uuid, resource_group: &str, provider: &str) -> Self {
        Self(format!(
            "/subscriptions/{subscription_id}/resourceGroups/{resource_group}/providers/{provider}"
        ))
    }

    #[must_use]
    pub fn is_subscription(&self) -> bool {
        self.0.starts_with("/subscriptions/") && !self.0.contains("/resourceGroups/")
    }

    #[must_use]
    pub fn subscription(&self) -> Option<Uuid> {
        let entries = self.0.split('/').collect::<Vec<_>>();
        let first = entries.get(1)?;
        if first != &"subscriptions" {
            return None;
        }
        let id = entries.get(2)?;
        Uuid::parse_str(id).ok()
    }
}

impl Display for Scope {
    fn fmt(&self, f: &mut Formatter<'_>) -> FmtResult {
        write!(f, "{}", self.0)
    }
}

impl FromStr for Scope {
    type Err = ScopeError;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Self::new(s.to_string())
    }
}
