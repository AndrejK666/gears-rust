use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct SettingsConfig {
    #[serde(default = "default_max_field_length")]
    pub max_field_length: usize,

    /// How many named settings one user may hold in one tenant.
    #[serde(default = "default_named_settings_per_user")]
    pub named_settings_per_user: usize,

    /// Upper bound on one named setting's value, as serialized JSON bytes.
    #[serde(default = "default_named_value_max_bytes")]
    pub named_value_max_bytes: usize,
}

impl Default for SettingsConfig {
    fn default() -> Self {
        Self {
            max_field_length: default_max_field_length(),
            named_settings_per_user: default_named_settings_per_user(),
            named_value_max_bytes: default_named_value_max_bytes(),
        }
    }
}

fn default_max_field_length() -> usize {
    100
}

fn default_named_settings_per_user() -> usize {
    256
}

fn default_named_value_max_bytes() -> usize {
    4096
}
