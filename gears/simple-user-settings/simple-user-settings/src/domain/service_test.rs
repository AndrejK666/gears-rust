//! Integration tests for the Settings service.
//!
//! These tests use an in-memory `SQLite` database since `DBRunner` is a sealed trait
//! and cannot be mocked. All tests use real database operations.

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use authz_resolver_sdk::{
        AuthZResolverApi, PolicyEnforcer,
        constraints::{Constraint, InPredicate, Predicate},
        models::{EvaluationRequest, EvaluationResponse, EvaluationResponseContext},
    };
    use simple_user_settings_sdk::models::{SimpleUserSettingsPatch, SimpleUserSettingsUpdate};
    use toolkit::api::canonical_prelude::CanonicalError;
    use toolkit_db::migration_runner::run_migrations_for_testing;
    use toolkit_db::{ConnectOpts, DBProvider, Db, connect_db};
    use toolkit_security::{PlatformSecurityContext, SecurityContext, pep_properties};
    use uuid::Uuid;

    use crate::domain::error::DomainError;
    use crate::domain::service::{Service, ServiceConfig};
    use crate::infra::storage::migrations::Migrator;
    use crate::infra::storage::sea_orm_repo::SeaOrmSettingsRepository;

    type ConcreteService = Service<SeaOrmSettingsRepository>;

    /// Mock `AuthZ` resolver for personal user settings.
    ///
    /// Derives tenant from `context.tenant_context.root_id` if present,
    /// otherwise falls back to `subject.properties.tenant_id` (like a real PDP).
    /// Always returns:
    /// - `OWNER_TENANT_ID` constraint from the resolved tenant
    /// - `RESOURCE_ID` constraint from `resource.id` (the user whose settings are accessed)
    struct MockAuthZResolver;

    #[async_trait]
    impl AuthZResolverApi for MockAuthZResolver {
        async fn evaluate(
            &self,
            _ctx: PlatformSecurityContext,
            request: EvaluationRequest,
        ) -> Result<EvaluationResponse, CanonicalError> {
            // Resolve tenant: explicit context > subject property (like a real PDP)
            let root_id = request
                .context
                .tenant_context
                .as_ref()
                .and_then(|tc| tc.root_id)
                .or_else(|| {
                    request
                        .subject
                        .properties
                        .get("tenant_id")
                        .and_then(|v| v.as_str())
                        .and_then(|s| Uuid::parse_str(s).ok())
                })
                .ok_or_else(|| {
                    CanonicalError::internal("tenant context is required".to_owned()).create()
                })?;

            let mut predicates = vec![Predicate::In(InPredicate::new(
                pep_properties::OWNER_TENANT_ID,
                [root_id],
            ))];

            // Use resource.id for RESOURCE_ID constraint
            if let Some(resource_id) = request.resource.id {
                predicates.push(Predicate::In(InPredicate::new(
                    pep_properties::RESOURCE_ID,
                    [resource_id],
                )));
            }

            Ok(EvaluationResponse {
                decision: true,
                context: EvaluationResponseContext {
                    constraints: vec![Constraint { predicates }],
                    ..Default::default()
                },
            })
        }
    }

    /// Create an in-memory database with migrations applied.
    async fn inmem_db() -> Db {
        use sea_orm_migration::MigratorTrait;

        let opts = ConnectOpts {
            max_conns: Some(1),
            min_conns: Some(1),
            ..Default::default()
        };
        let db = connect_db("sqlite::memory:", opts)
            .await
            .expect("Failed to connect to in-memory database");

        run_migrations_for_testing(&db, Migrator::migrations())
            .await
            .expect("Failed to run migrations");

        db
    }

    fn create_test_context() -> SecurityContext {
        SecurityContext::builder()
            .subject_id(Uuid::new_v4())
            .subject_tenant_id(Uuid::new_v4())
            .build()
            .unwrap()
    }

    fn build_service(db: Db, config: ServiceConfig) -> ConcreteService {
        let repo = Arc::new(SeaOrmSettingsRepository::new());
        let db: Arc<DBProvider<toolkit_db::DbError>> = Arc::new(DBProvider::new(db));
        let authz: Arc<dyn AuthZResolverApi> = Arc::new(MockAuthZResolver);
        let policy_enforcer = PolicyEnforcer::new(authz);
        Service::new(db, repo, policy_enforcer, config)
    }

    // =========================================================================
    // get_settings tests
    // =========================================================================

    #[tokio::test]
    async fn test_get_settings_returns_defaults_when_not_found() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        let result = service.get_settings(&ctx).await.unwrap();

        assert_eq!(result.user_id, ctx.subject_id());
        assert_eq!(result.tenant_id, ctx.subject_tenant_id());
        assert_eq!(result.theme, None);
        assert_eq!(result.language, None);
    }

    #[tokio::test]
    async fn test_get_settings_returns_existing() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        // First, create settings
        let _ = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();

        // Then retrieve them
        let result = service.get_settings(&ctx).await.unwrap();

        assert_eq!(result.theme, Some("dark".to_owned()));
        assert_eq!(result.language, Some("en".to_owned()));
    }

    // =========================================================================
    // update_settings tests
    // =========================================================================

    #[tokio::test]
    async fn test_update_settings_success() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        let result = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "light".to_owned(),
                    language: "es".to_owned(),
                },
            )
            .await
            .unwrap();

        assert_eq!(result.theme, Some("light".to_owned()));
        assert_eq!(result.language, Some("es".to_owned()));
        assert_eq!(result.user_id, ctx.subject_id());
        assert_eq!(result.tenant_id, ctx.subject_tenant_id());
    }

    #[tokio::test]
    async fn test_update_settings_validates_max_length_for_theme() {
        let db = inmem_db().await;
        let service = build_service(
            db,
            ServiceConfig {
                max_field_length: 10,
                ..ServiceConfig::default()
            },
        );
        let ctx = create_test_context();

        let too_long = "a".repeat(11);
        let result = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: too_long,
                    language: "en".to_owned(),
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DomainError::Validation { field, .. } if field == "theme"));
    }

    #[tokio::test]
    async fn test_update_settings_validates_max_length_for_language() {
        let db = inmem_db().await;
        let service = build_service(
            db,
            ServiceConfig {
                max_field_length: 10,
                ..ServiceConfig::default()
            },
        );
        let ctx = create_test_context();

        let too_long = "a".repeat(11);
        let result = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: too_long,
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DomainError::Validation { field, .. } if field == "language"));
    }

    // =========================================================================
    // patch_settings tests
    // =========================================================================

    #[tokio::test]
    async fn test_patch_settings_updates_only_provided_fields() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        // First create initial settings
        let _ = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();

        // Patch only theme
        let result = service
            .patch_settings(
                &ctx,
                SimpleUserSettingsPatch {
                    theme: Some("light".to_owned()),
                    language: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(result.theme, Some("light".to_owned()));
        assert_eq!(result.language, Some("en".to_owned())); // Should remain unchanged
    }

    #[tokio::test]
    async fn test_patch_settings_validates_max_length() {
        let db = inmem_db().await;
        let service = build_service(
            db,
            ServiceConfig {
                max_field_length: 10,
                ..ServiceConfig::default()
            },
        );
        let ctx = create_test_context();

        let too_long = "a".repeat(11);
        let result = service
            .patch_settings(
                &ctx,
                SimpleUserSettingsPatch {
                    theme: None,
                    language: Some(too_long),
                },
            )
            .await;

        assert!(result.is_err());
        let err = result.unwrap_err();
        assert!(matches!(err, DomainError::Validation { field, .. } if field == "language"));
    }

    #[tokio::test]
    async fn test_patch_settings_empty_patch_returns_existing() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        // First create settings
        let _ = service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();

        // Empty patch - no fields to update
        let result = service
            .patch_settings(
                &ctx,
                SimpleUserSettingsPatch {
                    theme: None,
                    language: None,
                },
            )
            .await
            .unwrap();

        // Should return existing values unchanged
        assert_eq!(result.theme, Some("dark".to_owned()));
        assert_eq!(result.language, Some("en".to_owned()));
    }

    #[tokio::test]
    async fn test_patch_settings_creates_if_not_exists() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());
        let ctx = create_test_context();

        // Patch without existing settings
        let result = service
            .patch_settings(
                &ctx,
                SimpleUserSettingsPatch {
                    theme: Some("dark".to_owned()),
                    language: None,
                },
            )
            .await
            .unwrap();

        assert_eq!(result.theme, Some("dark".to_owned()));
        assert_eq!(result.language, None);
    }

    // =========================================================================
    // Tenant isolation tests
    // =========================================================================

    #[tokio::test]
    async fn test_settings_isolated_by_user() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());

        let tenant_id = Uuid::new_v4();
        let user1 = SecurityContext::builder()
            .subject_id(Uuid::new_v4())
            .subject_tenant_id(tenant_id)
            .build()
            .unwrap();
        let user2 = SecurityContext::builder()
            .subject_id(Uuid::new_v4())
            .subject_tenant_id(tenant_id)
            .build()
            .unwrap();

        // User 1 creates settings
        let _ = service
            .update_settings(
                &user1,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();

        // User 2 should get default settings
        let result = service.get_settings(&user2).await.unwrap();
        assert_eq!(result.theme, None);
        assert_eq!(result.language, None);
        assert_eq!(result.user_id, user2.subject_id());
    }

    #[tokio::test]
    async fn test_settings_isolated_by_tenant() {
        let db = inmem_db().await;
        let service = build_service(db, ServiceConfig::default());

        let user_id = Uuid::new_v4();
        let tenant1 = SecurityContext::builder()
            .subject_id(user_id)
            .subject_tenant_id(Uuid::new_v4())
            .build()
            .unwrap();
        let tenant2 = SecurityContext::builder()
            .subject_id(user_id)
            .subject_tenant_id(Uuid::new_v4())
            .build()
            .unwrap();

        // Same user in tenant 1 creates settings
        let _ = service
            .update_settings(
                &tenant1,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .unwrap();

        // Same user in tenant 2 should get default settings
        let result = service.get_settings(&tenant2).await.unwrap();
        assert_eq!(result.theme, None);
        assert_eq!(result.language, None);
        assert_eq!(result.tenant_id, tenant2.subject_tenant_id());
    }

    // =========================================================================
    // named settings
    // =========================================================================

    async fn named_service(config: ServiceConfig) -> ConcreteService {
        build_service(inmem_db().await, config)
    }

    /// Any JSON value goes in and comes back out unchanged.
    #[tokio::test]
    async fn a_named_setting_round_trips_any_json() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();

        let values = [
            ("portal.projects.view", serde_json::json!("table")),
            ("portal.sidebar.width", serde_json::json!(280)),
            (
                "portal.hints.dismissed",
                serde_json::json!(["welcome", "tour"]),
            ),
            ("portal.editor", serde_json::json!({"wrap": true, "tab": 4})),
            ("portal.flag", serde_json::json!(null)),
        ];
        for (key, value) in &values {
            let stored = service
                .put_named_setting(&ctx, key, value.clone())
                .await
                .expect("stored");
            assert_eq!(&stored.value, value);
        }
        for (key, value) in &values {
            let seen = service
                .get_named_setting(&ctx, key)
                .await
                .expect("read")
                .expect("present");
            assert_eq!(&seen.value, value, "{key}");
        }
    }

    #[tokio::test]
    async fn a_named_setting_that_was_never_set_is_absent() {
        let service = named_service(ServiceConfig::default()).await;
        let seen = service
            .get_named_setting(&create_test_context(), "portal.projects.view")
            .await
            .expect("read");
        assert_eq!(seen, None);
    }

    #[tokio::test]
    async fn putting_a_named_setting_again_replaces_it() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();

        service
            .put_named_setting(&ctx, "portal.projects.view", serde_json::json!("table"))
            .await
            .expect("first");
        service
            .put_named_setting(&ctx, "portal.projects.view", serde_json::json!("tiles"))
            .await
            .expect("second");

        let all = service.list_named_settings(&ctx).await.expect("list");
        assert_eq!(all.len(), 1, "replaced, not duplicated");
        assert_eq!(all[0].value, serde_json::json!("tiles"));
    }

    #[tokio::test]
    async fn named_settings_list_in_key_order() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();
        for key in ["b.second", "c.third", "a.first"] {
            service
                .put_named_setting(&ctx, key, serde_json::json!(key))
                .await
                .expect("stored");
        }

        let keys: Vec<String> = service
            .list_named_settings(&ctx)
            .await
            .expect("list")
            .into_iter()
            .map(|s| s.key)
            .collect();
        assert_eq!(keys, ["a.first", "b.second", "c.third"]);
    }

    /// Deleting says whether there was anything to delete, and deleting twice
    /// is not an error.
    #[tokio::test]
    async fn deleting_a_named_setting_forgets_it() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();
        service
            .put_named_setting(&ctx, "portal.projects.view", serde_json::json!("table"))
            .await
            .expect("stored");

        let existed = service
            .delete_named_setting(&ctx, "portal.projects.view")
            .await
            .expect("deleted");
        assert!(existed);
        let again = service
            .delete_named_setting(&ctx, "portal.projects.view")
            .await
            .expect("deleting again is fine");
        assert!(!again);
        assert_eq!(
            service
                .get_named_setting(&ctx, "portal.projects.view")
                .await
                .expect("read"),
            None
        );
    }

    #[tokio::test]
    async fn malformed_keys_are_refused() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();
        let too_long = "k".repeat(129);

        for key in [
            "",
            "has space",
            "a/b",
            "caf\u{e9}",
            "a?b",
            too_long.as_str(),
        ] {
            let err = service
                .put_named_setting(&ctx, key, serde_json::json!(1))
                .await
                .expect_err("refused");
            assert!(
                matches!(&err, DomainError::Validation { field, .. } if field == "key"),
                "{key:?}: {err:?}"
            );
            assert!(
                service.get_named_setting(&ctx, key).await.is_err(),
                "reads validate too: {key:?}"
            );
        }

        let longest = "k".repeat(128);
        service
            .put_named_setting(&ctx, &longest, serde_json::json!(1))
            .await
            .expect("128 characters is allowed");
        service
            .put_named_setting(&ctx, "Portal_2.view-mode:v1", serde_json::json!(1))
            .await
            .expect("every allowed punctuation mark");
    }

    #[tokio::test]
    async fn a_named_value_over_the_size_bound_is_refused() {
        let service = named_service(ServiceConfig {
            named_value_max_bytes: 10,
            ..ServiceConfig::default()
        })
        .await;
        let ctx = create_test_context();

        // `"12345678"` is exactly 10 bytes as JSON, quotes included.
        service
            .put_named_setting(&ctx, "fits", serde_json::json!("12345678"))
            .await
            .expect("at the bound");
        let err = service
            .put_named_setting(&ctx, "too.big", serde_json::json!("123456789"))
            .await
            .expect_err("over the bound");
        assert!(
            matches!(&err, DomainError::Validation { field, .. } if field == "value"),
            "{err:?}"
        );
    }

    /// The count bound stops new keys, not replacements, and frees up again
    /// once a key is deleted.
    #[tokio::test]
    async fn the_named_setting_count_bound_applies_to_new_keys() {
        let service = named_service(ServiceConfig {
            named_settings_per_user: 2,
            ..ServiceConfig::default()
        })
        .await;
        let ctx = create_test_context();
        let one = serde_json::json!(1);

        service
            .put_named_setting(&ctx, "a", one.clone())
            .await
            .expect("first");
        service
            .put_named_setting(&ctx, "b", one.clone())
            .await
            .expect("second");

        let err = service
            .put_named_setting(&ctx, "c", one.clone())
            .await
            .expect_err("third new key");
        assert!(
            matches!(&err, DomainError::Validation { field, .. } if field == "key"),
            "{err:?}"
        );

        service
            .put_named_setting(&ctx, "a", serde_json::json!(2))
            .await
            .expect("replacing at the bound");

        service
            .delete_named_setting(&ctx, "b")
            .await
            .expect("delete");
        service
            .put_named_setting(&ctx, "c", one)
            .await
            .expect("room again after a delete");
    }

    #[tokio::test]
    async fn named_settings_are_isolated_by_user_and_by_tenant() {
        let service = named_service(ServiceConfig::default()).await;
        let org = Uuid::new_v4();
        let person = Uuid::new_v4();
        let caller = |subject, tenant| {
            SecurityContext::builder()
                .subject_id(subject)
                .subject_tenant_id(tenant)
                .build()
                .unwrap()
        };
        let owner = caller(person, org);
        let colleague = caller(Uuid::new_v4(), org);
        let owner_elsewhere = caller(person, Uuid::new_v4());

        service
            .put_named_setting(&owner, "portal.projects.view", serde_json::json!("table"))
            .await
            .expect("stored");

        for other in [&colleague, &owner_elsewhere] {
            assert!(
                service
                    .list_named_settings(other)
                    .await
                    .expect("list")
                    .is_empty()
            );
            assert_eq!(
                service
                    .get_named_setting(other, "portal.projects.view")
                    .await
                    .expect("read"),
                None
            );
            assert!(
                !service
                    .delete_named_setting(other, "portal.projects.view")
                    .await
                    .expect("delete"),
                "cannot delete someone else's setting"
            );
        }

        assert!(
            service
                .get_named_setting(&owner, "portal.projects.view")
                .await
                .expect("read")
                .is_some(),
            "still there for the owner"
        );
    }

    /// Named settings live beside the fixed fields and leave them alone.
    #[tokio::test]
    async fn named_settings_do_not_touch_theme_and_language() {
        let service = named_service(ServiceConfig::default()).await;
        let ctx = create_test_context();
        service
            .update_settings(
                &ctx,
                SimpleUserSettingsUpdate {
                    theme: "dark".to_owned(),
                    language: "en".to_owned(),
                },
            )
            .await
            .expect("fixed fields");

        service
            .put_named_setting(&ctx, "theme", serde_json::json!("light"))
            .await
            .expect("a named key may share a fixed field's name");

        let fixed = service.get_settings(&ctx).await.expect("read");
        assert_eq!(fixed.theme.as_deref(), Some("dark"));
    }

    /// A PDP that clamps to the caller's tenant and nothing more, as the
    /// platform's static-authz plugin does. Which user's rows these are is the
    /// gear's to pin, not the PDP's.
    struct TenantOnlyAuthZ;

    #[async_trait]
    impl AuthZResolverApi for TenantOnlyAuthZ {
        async fn evaluate(
            &self,
            _ctx: PlatformSecurityContext,
            request: EvaluationRequest,
        ) -> Result<EvaluationResponse, CanonicalError> {
            let tenant = request
                .subject
                .properties
                .get("tenant_id")
                .and_then(|v| v.as_str())
                .and_then(|s| Uuid::parse_str(s).ok())
                .ok_or_else(|| CanonicalError::internal("no tenant".to_owned()).create())?;
            Ok(EvaluationResponse {
                decision: true,
                context: EvaluationResponseContext {
                    constraints: vec![Constraint {
                        predicates: vec![Predicate::In(InPredicate::new(
                            pep_properties::OWNER_TENANT_ID,
                            [tenant],
                        ))],
                    }],
                    ..Default::default()
                },
            })
        }
    }

    #[tokio::test]
    async fn under_a_tenant_wide_grant_named_settings_stay_the_callers_own() {
        let repo = Arc::new(SeaOrmSettingsRepository::new());
        let db: Arc<DBProvider<toolkit_db::DbError>> = Arc::new(DBProvider::new(inmem_db().await));
        let authz: Arc<dyn AuthZResolverApi> = Arc::new(TenantOnlyAuthZ);
        let service = Service::new(
            db,
            repo,
            PolicyEnforcer::new(authz),
            ServiceConfig {
                named_settings_per_user: 1,
                ..ServiceConfig::default()
            },
        );
        let org = Uuid::new_v4();
        let caller = |subject| {
            SecurityContext::builder()
                .subject_id(subject)
                .subject_tenant_id(org)
                .build()
                .unwrap()
        };
        let me = caller(Uuid::from_u128(1));
        let colleague = caller(Uuid::from_u128(2));

        service
            .put_named_setting(&me, "portal.projects.view", serde_json::json!("table"))
            .await
            .expect("stored");

        assert!(
            service
                .list_named_settings(&colleague)
                .await
                .expect("list")
                .is_empty()
        );
        assert_eq!(
            service
                .get_named_setting(&colleague, "portal.projects.view")
                .await
                .expect("read"),
            None
        );
        assert!(
            !service
                .delete_named_setting(&colleague, "portal.projects.view")
                .await
                .expect("delete"),
            "the colleague's delete does not reach my row"
        );
        service
            .put_named_setting(&colleague, "other.key", serde_json::json!(1))
            .await
            .expect("my row does not count against the colleague's bound");

        assert!(
            service
                .get_named_setting(&me, "portal.projects.view")
                .await
                .expect("read")
                .is_some()
        );
    }
}
