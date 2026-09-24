use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsConfig {
    #[serde(default = "default_max_field_length")]
    pub max_field_length: usize,

    /// How long one call to the deployment's `SettingsOwnerResolver` may take
    /// before the request fails with `ServiceUnavailable`. Unused when no
    /// resolver is registered.
    #[serde(default = "default_owner_resolver_timeout_ms")]
    pub owner_resolver_timeout_ms: u64,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            max_field_length: default_max_field_length(),
            owner_resolver_timeout_ms: default_owner_resolver_timeout_ms(),
        }
    }
}

fn default_max_field_length() -> usize {
    100
}

fn default_owner_resolver_timeout_ms() -> u64 {
    2000
}
