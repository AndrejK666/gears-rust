# API Gateway Gear

HTTP gateway gear that owns the Axum router and collects typed operation specs to emit a single OpenAPI document.

## Overview

The `cf-gears-api-gateway` crate provides:

- HTTP server host for REST APIs
- Operation registration via `toolkit::api::OperationBuilder`
- OpenAPI document aggregation

## Configuration

```yaml
gears:
  api_gateway:
    config:
      bind_addr: "127.0.0.1:8086"
      enable_docs: true
      cors_enabled: false
      auth_disabled: false
```

### Ordering the documentation groups

An operation declares the tag it belongs to; `openapi.tags` declares the groups
those tags name — the order a reader meets them in, and what each one is for.
This is the assembly's call rather than any single gear's: an assembly links
gears it does not own, so the tags in its document come from several authors and
none of them can decide the order.

```yaml
gears:
  api_gateway:
    config:
      enable_docs: true
      openapi:
        title: "Example Assembly"
        version: "0.1.0"
        tags:
          - name: Orders
            description: "Placing an order, and everything that happens to it after."
          - name: Tenants
```

Omit the key and the document carries no `tags` list, which is what it did
before the key existed: documentation browsers then fall back to the order the
tags first appear in, which in an assembly is whatever the path alphabet
produced. A tag an operation uses but the list omits is not hidden — it is
grouped as it always was, after the ones named here, so the list only has to
name the groups whose placement matters.

## License

Licensed under Apache-2.0.
