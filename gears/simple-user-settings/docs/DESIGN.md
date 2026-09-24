# DESIGN

## 1. Architecture Overview

Simple User Settings is implemented as a ToolKit gear with database persistence. It provides a REST API for CRUD operations on user settings.

**System Context**: Operates as a lightweight service gear within Gears, using the platform's database layer for storage.

## 2. Design Principles

### Simplicity

**ID**: [ ] `p2` `fdd-user-settings-principle-simplicity-v1`

<!-- fdd-id-content -->
Minimal API surface. No complex query language. Straightforward key-value model.
<!-- fdd-id-content -->

### Security First

**ID**: [ ] `p2` `fdd-user-settings-principle-security-v1`

<!-- fdd-id-content -->
Tenant isolation enforced at DB layer. User authentication required. No anonymous access.
<!-- fdd-id-content -->

## 3. Constraints

### Data Size

**ID**: [ ] `p2` `fdd-user-settings-constraint-size-v1`

<!-- fdd-id-content -->
Maximum 1MB per user settings document. Individual key-value pairs limited to 100KB.
<!-- fdd-id-content -->

### Schema

**ID**: [ ] `p2` `fdd-user-settings-constraint-schema-v1`

<!-- fdd-id-content -->
Free-form JSON storage. No enforced schema validation. Application responsible for data structure.
<!-- fdd-id-content -->

## 4. Components

### REST Endpoints

**ID**: [ ] `p1` `fdd-user-settings-component-rest-v1`

<!-- fdd-id-content -->
Fixed fields (`theme`, `language`):
- `GET /simple-user-settings/v1/settings` - Retrieve the fixed fields (defaults when unset)
- `POST /simple-user-settings/v1/settings` - Replace both fields
- `PATCH /simple-user-settings/v1/settings` - Update the fields given

Named settings (any key, JSON value):
- `GET /simple-user-settings/v1/named-settings` - Retrieve all named settings, ordered by key
- `GET /simple-user-settings/v1/named-settings/{key}` - Retrieve one; 404 if unset
- `PUT /simple-user-settings/v1/named-settings/{key}` - Create or replace one
- `DELETE /simple-user-settings/v1/named-settings/{key}` - Delete one; 204 whether or not it was set
<!-- fdd-id-content -->

### Settings Service

**ID**: [ ] `p1` `fdd-user-settings-component-service-v1`

<!-- fdd-id-content -->
Handles CRUD operations. Enforces tenant scoping. Validates request payloads.
<!-- fdd-id-content -->

### Database Repository

**ID**: [ ] `p1` `fdd-user-settings-component-repository-v1`

<!-- fdd-id-content -->
Persists settings to database. Uses toolkit-db for database access. Implements tenant isolation via security context.
<!-- fdd-id-content -->

## 5. Data Model

**`settings`** (fixed fields, one row per user and tenant):
- `tenant_id`, `user_id`: primary key `(tenant_id, user_id)`
- `theme`, `language`: nullable text

**`named_settings`** (one row per user, tenant and key):
- `tenant_id`, `user_id`, `key`: primary key `(tenant_id, user_id, key)`
- `value`: the JSON value, serialized to text on every backend

Both tables are scoped the same way (tenant column `tenant_id`, resource column
`user_id`) and authorized as the same PDP resource, `simple_user_settings.settings`:
reads need `get`, writes and deletes need `update`.

**Bounds on named settings** (configurable):
- key: 1–128 characters from `A–Z a–z 0–9 . _ - :`
- value: `named_value_max_bytes` as serialized JSON (default 4096)
- count: `named_settings_per_user` per user and tenant (default 256); applies to
  new keys only, so replacing a value at the bound still works

## 6. Sequences

### Settings Operation Flow

**ID**: [ ] `p1` `fdd-user-settings-seq-operation-v1`

<!-- fdd-id-content -->
1. Client sends authenticated request with tenant context
2. API layer validates authentication and authorization
3. Settings service applies tenant scoping
4. Repository queries/updates database with security context
5. Response returned to client

**Components**: `fdd-user-settings-component-rest-v1`, `fdd-user-settings-component-service-v1`, `fdd-user-settings-component-repository-v1`
<!-- fdd-id-content -->

## 7. Error Handling

- Unauthenticated request → 401 Unauthorized
- Out of the caller's scope → 404 Not Found (masked, so existence is not disclosed)
- Named setting not set → 404 Not Found
- Malformed key, oversized value, or a new key past the count bound → 400 Bad Request
  with a field violation on `key` or `value`

## 8. Dependencies

- toolkit-db for database access
- toolkit-auth for authentication/authorization
- toolkit-security for tenant context

## Appendix

### Change Log

| Date | Version | Author | Changes |
|------|---------|--------|---------|
| 2026-02-09 | 0.1.0 | System | Initial DESIGN for cypilot validation |
| 2026-09-24 | 0.2.0 | Andrej Kuchma | Named settings; data model and error handling brought in line with the code |
