# ADR-0013: Admin API on a Separate Internal Listener

## Status

Accepted

## Context

FR-13 requires all `/admin/*` endpoints to not be exposed on the public-facing
CDN port. Admin endpoints manage embargoes and presets; exposing them to the
public internet — even behind authentication — increases the attack surface
unnecessarily.

Two implementation strategies were evaluated:

| Criterion | Two `TcpListener` binds | Single listener, path-based split |
|---|---|---|
| FR-13 compliance | Strong — OS-level port boundary | Weak — relies on Nginx / NetworkPolicy |
| Operational clarity | `:3001` is always internal-only | Requires external config to block `/admin/*` |
| Code complexity | Two `axum::serve` tasks | One `serve`; auth middleware on `/admin/*` |
| Defense in depth | Higher | Lower |

## Decision

Bind the admin router to a **separate `TcpListener`** on `RENDITION_ADMIN_BIND_ADDR`
(default: `127.0.0.1:3001`). The CDN router binds on `RENDITION_BIND_ADDR`
(default: `0.0.0.0:3000`). Both listeners run as concurrent `tokio::spawn` tasks
within the same process.

```text
main()
 ├── tokio::spawn → axum::serve(cdn_listener, cdn_router)
 └── tokio::spawn → axum::serve(admin_listener, admin_router)
```

The Kubernetes `Service` manifest exposes port `3000` externally and port `3001`
only within the cluster (or not at all — internal admin tooling connects directly
to the pod via `kubectl port-forward` or a ClusterIP service).

## Consequences

**Benefits:**

- The OS-level port separation means no application-layer misconfiguration can
  accidentally expose admin endpoints to CDN traffic. The boundary is enforced
  by the TCP stack, not by middleware ordering.
- Kubernetes `NetworkPolicy` and `Service` definitions explicitly map the two
  ports to two distinct exposure levels, making the security posture clear in
  infrastructure-as-code.
- `axum-test` can spin up both listeners independently in integration tests.

**Drawbacks:**

- Two `axum::serve` calls and two graceful shutdown handles to manage. Mitigated
  by a shared `tokio::sync::CancellationToken` passed to both tasks.
- Operators must configure both `RENDITION_BIND_ADDR` and
  `RENDITION_ADMIN_BIND_ADDR` in their deployment manifests.
