# Audit log and signed webhooks

Workspace administrators can inspect **Manage → Audit log** and configure **Manage → Webhooks**. Named-feed audit views filter to the current feed. Webhook administration is workspace-wide; an optional feed ID restricts a subscription.

Management mutations create an audit attempt before executing. The record captures the actor/profile key ID, feed, method, path, event type, sanitized target ID/version and HTTP result. Authorization headers, request bodies, URL query strings and returned API keys are never stored. An interrupted process can leave a pending record; a completed action whose audit finalization failed also remains pending and is logged for reconciliation. Audit records have no edit/delete API.

Successful HTTP mutations enqueue matching webhook deliveries when the audit result is finalized. Events include `package.published`, `package.unlisted`, `package.relisted`, `symbols.published`, `symbols.deleted`, `credential.created`, `credential.revoked`, `upstream.changed`, `cache.changed` and `management.changed`. Scheduled maintenance and advisory sync are reported by their own status screens and do not emit HTTP mutation events.

## Delivery and verification

Configure a dedicated environment secret such as `WEBHOOK_RELEASE_SIGNING_KEY` (at least 32 characters). The UI/API receives only the environment variable's name. Forward that variable into the app container using your deployment secret mechanism.

| Header | Meaning |
| --- | --- |
| `X-NuShelf-Delivery` | Stable UUID; deduplicate retries using this value |
| `X-NuShelf-Event` | Event type |
| `X-NuShelf-Timestamp` | Unix timestamp for this attempt |
| `X-NuShelf-Signature` | `sha256=<hex HMAC-SHA256>` |

Verify HMAC-SHA256 over `<timestamp>.<exact request body bytes>`, compare in constant time, and reject stale timestamps. Return 2xx after accepting the event. Delivery is **at least once**, including when acceptance succeeds but its response is lost. Six attempts use increasing backoff; the screen shows attempts, status and errors. PostgreSQL row locks coordinate replicas. Disabling a hook cancels pending deliveries. Configuration commands wait for any in-flight attempt to finish before taking effect.

HTTPS is required by default. Development HTTP and private network access require explicit settings. DNS addresses are validated and pinned per request; redirects and environment proxies are disabled. URLs must not contain credentials, queries or fragments. Receivers get sanitized event metadata, never packages or credentials. An administrator-selected receiver can receive private-feed event metadata, so restrict subscriptions appropriately.

APIs: `GET /api/events?take=100&skip=0`, `GET/POST /api/webhooks`, `DELETE /api/webhooks/<id>` (disable), `GET /api/webhooks/deliveries`, and `POST /api/webhooks/dispatch` (process due deliveries). Supply `events: ["*"]` for all events, or select specific types; supply `feedId` for one feed. Use `PUT /api/webhooks/<id>` with the complete configuration and the `revision` returned by the list API to edit or re-enable a receiver. A stale revision returns 409; reload before saving again. Unknown feed IDs return 400. Changing only the name preserves pending deliveries. Changing the URL, secret environment reference, network permissions, event types or feed cancels pending deliveries. Re-enabling does not resurrect old deliveries.

Use `POST /api/webhooks/deliveries/<id>/retry` to queue a failed or cancelled delivery (202). The receiver must be enabled and the event must match its current subscription. Pending and successful deliveries cannot be retried (409). The stable delivery UUID and event payload are preserved; the next attempt uses the current receiver configuration and a fresh timestamp/signature. Each manual retry opens another six-attempt cycle with increasing backoff. Total attempts and manual retry counts remain cumulative.

`GET /api/webhooks/deliveries/<id>/attempts` returns individual attempts with receiver URL, configuration revision, timestamp, HTTP result and sanitized error. It never returns secrets or response bodies. Attempts made before this feature was installed retain their aggregate counters; detailed history starts after installation. All webhook endpoints require a workspace administrator credential, including on named-feed routes.
