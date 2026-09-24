use async_trait::async_trait;
use simple_user_settings_sdk::models::{SimpleUserSettings, SimpleUserSettingsPatch};
use toolkit::domain::DomainModel;
use toolkit_db::secure::DBRunner;
use toolkit_security::AccessScope;
use uuid::Uuid;

use super::error::DomainError;

#[async_trait]
pub trait SettingsRepository: Send + Sync
where
    SimpleUserSettings: DomainModel,
    SimpleUserSettingsPatch: DomainModel,
{
    /// The settings row of `user_id`, within `scope`.
    ///
    /// The scope is what the PDP allows; it need not name the user. A grant of
    /// "settings in this tenant" is a legitimate answer, so the row is always
    /// narrowed to the caller's own key here rather than left to the PDP.
    async fn find_by_user<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        user_id: Uuid,
    ) -> Result<Option<SimpleUserSettings>, DomainError>;

    async fn upsert_full<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        user_id: Uuid,
        tenant_id: Uuid,
        theme: Option<String>,
        language: Option<String>,
    ) -> Result<SimpleUserSettings, DomainError>;

    async fn upsert_patch<C: DBRunner>(
        &self,
        conn: &C,
        scope: &AccessScope,
        user_id: Uuid,
        tenant_id: Uuid,
        patch: SimpleUserSettingsPatch,
    ) -> Result<SimpleUserSettings, DomainError>;
}
