# ADR-0017: `prometheus` Crate for Metrics

## Status

Accepted

## Context

NFR-05 requires a `GET /metrics` endpoint in Prometheus text format exposing
request counts, error counts, cache hit/miss ratio, and transform latency
histograms.

Two approaches were evaluated:

| Criterion | `prometheus` crate | `metrics` + `metrics-exporter-prometheus` |
|---|---|---|
| Lines to register a counter | ~5 | ~3 |
| Backend swappability | No | Yes (swap exporter) |
| Histogram bucket config | Manual | Declarative |
| Maturity | Very high (official client) | High |
| Extra dependencies | 1 | 2 |
| v1 need for multiple backends | No | Premature |

## Decision

Use the **`prometheus` crate** (the official Prometheus Rust client) with a
global `Registry`.

Metrics registered at startup via `lazy_static!`:

```rust
// Counters
rendition_requests_total{method, path_prefix, status}
rendition_cache_hits_total
rendition_cache_misses_total
rendition_embargo_rejections_total
rendition_storage_errors_total{backend}

// Histograms
rendition_transform_duration_seconds{format}
rendition_storage_fetch_duration_seconds{backend}

// Gauges
rendition_cache_entries
rendition_circuit_breaker_open{backend}
```

The `GET /metrics` handler calls `prometheus::gather()` and renders the registry
in Prometheus text exposition format (content type
`text/plain; version=0.0.4; charset=utf-8`).

## Consequences

**Benefits:**

- The `prometheus` crate is the official Rust client maintained alongside the
  Prometheus project. It is battle-tested in production at scale.
- `prometheus::gather()` renders all metrics in a single call with no
  serialisation surprises.
- Direct control over histogram bucket boundaries allows tuning for Rendition's
  latency distribution (e.g. 1 ms, 5 ms, 10 ms, 50 ms, 100 ms, 500 ms buckets
  for transform duration).

**Drawbacks:**

- Prometheus is the only supported backend. If Rendition ever needs to emit to
  StatsD, OpenTelemetry metrics, or Datadog, migrating to the `metrics` facade
  is the natural path. The migration is mechanical (replace `prometheus::Counter`
  with `metrics::counter!` macro calls) and can be deferred until a second
  backend is needed.
- `lazy_static!` registration is verbose compared to the `metrics` macro API.
  This is acceptable given the small number of metrics in v1.
