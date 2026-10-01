# Simple User Settings SDK

SDK crate for the simple user settings gear.

## Overview

The `cf-gears-simple-user-settings-sdk` crate provides:

- `SimpleUserSettingsClientV1` trait — the fixed `theme` and `language`
- `NamedSettingsClientV1` trait — any number of keyed JSON values
- Model types (`SimpleUserSettings`, `SimpleUserSettingsPatch`, `SimpleUserSettingsUpdate`, `NamedSetting`)

Consumers obtain the clients from `ClientHub`.

```rust,ignore
use simple_user_settings_sdk::SimpleUserSettingsClientV1;

let client = hub.get::<dyn SimpleUserSettingsClientV1>()?;
let settings = client.get_settings(&ctx).await?;
```

## Named settings

For any preference beyond `theme` and `language`: a key the product chooses and a
JSON value, filed under the same user and tenant as the fixed fields.

```rust,ignore
use serde_json::json;
use simple_user_settings_sdk::NamedSettingsClientV1;

let named = hub.get::<dyn NamedSettingsClientV1>()?;
named.put_named_setting(&ctx, "portal.projects.view", json!("table")).await?;
let view = named.get_named_setting(&ctx, "portal.projects.view").await?; // Option<NamedSetting>
let all = named.list_named_settings(&ctx).await?;                        // ordered by key
named.delete_named_setting(&ctx, "portal.projects.view").await?;         // true if it existed
named.delete_all_named_settings(&ctx).await?;                             // how many were removed
```

Keys are 1–128 characters from `A–Z a–z 0–9 . _ - :`; namespace them yourself
(a dotted product prefix is the usual shape). Values and the number of keys per
user are bounded by the gear's configuration.

JSON `null` is a value like any other: it is stored, takes a slot of the key
count, and reads back as `Some` with a `null` value (`200` over REST). To unset
a key, delete it; a key that is not set reads back as `None` (`404`).

The store is one per user and tenant, shared by every product in the deployment
that writes to it. There is no per-product namespace, quota or policy: the key
count bound (`named_settings_per_user`, default 256) is one budget for all of
them, and every key is authorized as the same resource as `theme`/`language`.
It is meant for small UI preferences. A product that needs its own quota, or a
different access policy for its keys, needs its own store. Over REST the same operations are
`GET|PUT|DELETE /simple-user-settings/v1/named-settings/{key}` and
`GET|DELETE /simple-user-settings/v1/named-settings` (list all, delete all).

## Whose settings: `SettingsOwnerResolver`

Settings are filed under `(user, tenant)`, and by default the user is the token
subject (`ctx.subject_id()`). A deployment where one person can sign in through
more than one identity — an e-mail login and a brokered GitHub login, merged
accounts, an IdP migration — can register a `SettingsOwnerResolver` on the
`ClientHub` so that all of a person's logins share one set of settings:

```rust,ignore
use simple_user_settings_sdk::SettingsOwnerResolver;

let resolver: Arc<dyn SettingsOwnerResolver> = Arc::new(MyResolver);
hub.register(resolver); // in your gear's `init`
```

- With none registered, nothing changes: the subject is the key.
- The resolver returns `Ok(None)` for a caller it has no opinion about, and the
  subject is used; an `Err` fails the request instead of guessing.
- It answers the user half only, so logins are unified within a tenant, not
  across tenants.
- Its answer decides whose settings a request reads and writes and is what
  `SimpleUserSettings::user_id` reports. The trait docs spell out the contract
  (same person, same tenant, stable answers, cheap, cancel-safe) and how to
  re-key rows written before it was enabled.

## License

Licensed under Apache-2.0.
