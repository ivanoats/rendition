# Unit 2 — S3 Storage Backend — Deployment Architecture

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-infrastructure-design-plan.md`
**Companion:** `infrastructure-design.md` (bucket, IAM, network spec)

This document lays out the **topology** — how Rendition's S3 storage path
fits into the broader deployment footprint — and makes the multi-cloud
extension points explicit.

---

## Environment tiers

Three isolated tiers, each a separate AWS account (Q7=A):

| Tier | AWS account | EKS cluster | S3 bucket | Purpose |
|---|---|---|---|---|
| `dev` | `rendition-dev` | `rendition-dev-eks` | `rendition-dev-assets` | Internal dev experimentation; can run LocalStack for cargo tests |
| `staging` | `rendition-staging` | `rendition-staging-eks` | `rendition-staging-assets` | Pre-production validation; mirrors prod topology |
| `prod` | `rendition-prod` | `rendition-prod-eks` | `rendition-prod-assets` | Public-facing CDN backend |

**Account boundary = blast radius boundary.** A bug, leaked credential, or
misconfigured IAM role in `dev` cannot reach `prod` data.

**Fallback if multi-account is infeasible:** three separate buckets in one
account with environment-scoped IAM roles. Documented as a temporary
migration state in NFR plan Q7 fallback text.

---

## Topology diagram (AWS happy path, single environment)

```text
                        ┌──────────────────┐
                        │   Internet       │
                        │   (end users)    │
                        └────────┬─────────┘
                                 │ HTTPS
                                 ▼
                        ┌──────────────────┐
                        │  CloudFront /    │
                        │  Fastly / CDN    │   (upstream of Rendition,
                        └────────┬─────────┘    not provisioned here)
                                 │ HTTPS
                                 ▼
        ┌────────────────────────────────────────────────┐
        │  AWS VPC (private subnets, 3 AZs)              │
        │  ┌──────────────────────────────────────────┐  │
        │  │  EKS cluster                             │  │
        │  │  ┌────────────────┐  ┌────────────────┐  │  │
        │  │  │  Rendition pod │  │  Rendition pod │  │  │
        │  │  │  ─────────────│  │  ─────────────│  │  │
        │  │  │  S3Storage    ──┼──┼─► circuit    │  │  │
        │  │  │               │  │     breaker   │  │  │
        │  │  └──────┬─────────┘  └──────┬───────┘  │  │
        │  │         │                   │           │  │
        │  │         │  IRSA service acct│           │  │
        │  │         └────────┬──────────┘           │  │
        │  └──────────────────┼────────────────────┘    │
        │                     │ sts:AssumeRoleWithWebIdentity │
        │                     ▼                              │
        │  ┌──────────────────────────────────────────┐      │
        │  │ VPC gateway endpoint                     │      │
        │  │ com.amazonaws.{region}.s3                │──────┼───▶ S3
        │  │ (free; restricted by endpoint policy)    │      │    bucket
        │  └──────────────────────────────────────────┘      │   SSE-S3
        │                                                    │   BlockPublic
        └────────────────────────────────────────────────────┘
```

**Traffic never leaves the VPC** to reach S3. No NAT egress. No public
internet. Bucket is fronted by IAM + endpoint policy + bucket policy (three
layers of access control).

---

## Data flow — happy path `get` request

Numbered for reference in Unit 4's request handler:

1. End user GETs `/cdn/products/shoe.jpg?wid=800` (via CloudFront).
2. CloudFront forwards to Rendition EKS service (private ALB → pod).
3. Rendition pod's request handler resolves the asset path to
   `"products/shoe.jpg"` and calls
   `storage.get("products/shoe.jpg").await`.
4. `S3Storage` composes the key: `"v1/products/shoe.jpg"` (prefix +
   normalised path).
5. `CircuitBreaker::call` checks state (Closed) and permits the call.
6. `with_retries` wraps `tokio::time::timeout(5s, client.get_object(...))`.
7. AWS SDK signs the request using IRSA-provided credentials, sends it
   through the pod's ENI to the VPC gateway endpoint to S3.
8. S3 returns `200 OK` with the object body.
9. `ByteStream::collect` materialises bytes; `resolve_content_type`
   picks `"image/jpeg"` from the `Content-Type` header.
10. `S3Storage::get` returns `Ok(Asset)`.
11. Rendition's transform pipeline runs on the bytes and returns the
    resized image to the caller.

**Latency budget (NFR target):** P99 ≤ 50 ms for step 5 → step 10
(warm pool, intra-region).

---

## Failure scenarios

### F1 — Sustained S3 outage

1. Successive `GetObject` calls start failing with 503 Service
   Unavailable.
2. `with_retries` exhausts 3 attempts per call and raises
   `StorageError::Unavailable`.
3. `CircuitBreaker` counts each exhausted sequence as one failure.
4. After 5 consecutive failures (`s3_cb_threshold`), the breaker
   transitions to `Open`.
5. Subsequent calls return `StorageError::CircuitOpen` instantly without
   touching S3.
6. `/health/ready` (Unit 7) reads `!is_healthy()` and returns 503.
7. Kubernetes readiness probe detects the failure, stops routing new
   traffic to this pod. The pod continues processing in-flight requests
   but drains quickly.
8. After 30 s cooldown, the next call enters `HalfOpen`, probes S3.
9. If S3 has recovered, the probe succeeds and `/health/ready` flips
   back to 200; Kubernetes resumes routing.
10. If the probe fails, the breaker returns to `Open` with a fresh
    30 s cooldown.

### F2 — Transient blip

1. A single `GetObject` fails with `InternalError` (5xx).
2. `with_retries` retries after ~50 ms of full-jitter backoff.
3. The retry succeeds.
4. `CircuitBreaker` sees the sequence as a **success** (end result was
   `Ok`) and does not increment `consecutive_failures`.
5. End user sees a ~50 ms latency bump for one request. No cascade.

### F3 — Expired credentials (IRSA refresh)

1. IRSA-provided STS credentials expire every ~1 hour.
2. The AWS SDK's credential cache auto-refreshes them on the next call.
3. A refresh in flight may cause a single extra round trip to STS.
4. In the worst case, one `GetObject` fails with
   `ExpiredToken`, retries, and succeeds on the second attempt after
   the credential cache refreshes.
5. Credential expiration is **invisible to `S3Storage`** — handled
   entirely inside the SDK.

### F4 — Endpoint policy misconfiguration

1. Operator accidentally restricts the VPC endpoint policy to the
   wrong bucket ARN.
2. `GetObject` calls fail with `AccessDenied`.
3. R-01 classifies `AccessDenied` as **terminal, non-retriable** — the
   retry loop exits immediately.
4. Returned to caller as `StorageError::NotFound` (per R-01's
   403→NotFound mapping rule, which treats access-denied as
   indistinguishable from missing object).
5. End user sees 404. Rendition operator sees the actual failure in
   server-side logs (via `tracing::error!` with full cause chain).

This shows why R-01's "force 403 → NotFound at the boundary, but log
the real cause server-side" rule matters — it preserves information
asymmetry (users don't learn about bucket config) without blinding
operators.

---

## Multi-cloud deployment sketches

These are **not shipped** in Unit 2 — included as documentation so the
pluggability story is concrete.

### GCP deployment (future L2 implementation)

```text
┌──────────────────────────────────────────────┐
│  GCP VPC                                     │
│  ┌────────────────────────────────────────┐  │
│  │  GKE cluster                           │  │
│  │  ┌───────────────┐                     │  │
│  │  │ Rendition pod │                     │  │
│  │  │ GcsStorage ──┼──▶ Workload Identity │  │
│  │  └───────────────┘   (GCP service acct)│  │
│  │                                         │  │
│  └───────────────┬────────────────────────┘  │
│                  │ Private Google Access     │
│                  ▼                            │
│              GCS bucket                       │
│              (Google-managed encryption)      │
└──────────────────────────────────────────────┘
```

**Code change:** new `src/storage/gcs.rs` implementing
`StorageBackend`. The `CircuitBreaker` and `StorageError` types are
reused unchanged.

### Azure deployment (future L2 implementation)

Analogous — `AzureBlobStorage` on AKS with Azure AD Workload Identity,
private endpoint for Blob Storage, RBAC `Storage Blob Data Reader`
role. Same trait, same breaker, same error variants.

### S3-compatible cloud (today, code-path reuse only)

```text
┌─────────────────────────────────────────────┐
│  Any Kubernetes cluster                     │
│  ┌────────────────────────────────────┐     │
│  │  Rendition pod                     │     │
│  │  S3Storage                         │     │
│  │   with RENDITION_S3_ENDPOINT=      │     │
│  │   https://{account}.r2.cloudflare- │     │
│  │   storage.com                      │     │
│  │   credentials from K8s Secret      │     │
│  └──────────┬─────────────────────────┘     │
│             │ HTTPS (public)                 │
│             ▼                                 │
│       Cloudflare R2 bucket                   │
└─────────────────────────────────────────────┘
```

**Code change:** none. Same `S3Storage`, same `CircuitBreaker`.
Caveats noted in `infrastructure-design.md` for R2-specific
path-style addressing and credential delivery via K8s Secret
(less secure than IRSA but necessary outside AWS).

---

## Operator's deployment checklist (AWS happy path)

For each environment:

- [ ] Create AWS account or ensure one exists (Q7=A)
- [ ] Create EKS cluster with OIDC provider enabled (for IRSA)
- [ ] Create S3 bucket `rendition-{env}-assets` in the chosen region
  - [ ] Set `BucketEncryption: AES256` (SSE-S3)
  - [ ] Enable all four `PublicAccessBlock` flags
  - [ ] Set `ObjectOwnership: BucketOwnerEnforced`
  - [ ] Apply the deny-unencrypted-PutObject bucket policy
  - [ ] Set lifecycle rule: abort incomplete multipart uploads after 7 days
  - [ ] Tag the bucket with `Environment={env}`
- [ ] Create IAM role `rendition-{env}-s3-reader`
  - [ ] Trust policy: IRSA OIDC for the Rendition service account
  - [ ] Permissions policy: `s3:GetObject`, `s3:GetObjectVersion`,
    `s3:HeadObject`, `s3:ListBucket` on the bucket ARN + prefix,
    with `Environment` tag condition
- [ ] Create VPC gateway endpoint for S3 in the Rendition VPC
  - [ ] Attach to private subnet route tables
  - [ ] Apply endpoint policy scoped to the Rendition bucket ARN
- [ ] Create Rendition Kubernetes `ServiceAccount` annotated with the
  IAM role ARN
- [ ] Deploy Rendition with the env vars listed in
  `infrastructure-design.md` — no AWS credentials needed
- [ ] Verify:
  - [ ] A test `GetObject` from a Rendition pod succeeds
  - [ ] `/health/live` returns 200
  - [ ] `/health/ready` returns 200
  - [ ] CloudWatch metrics appear for the bucket

This list is the happy-path "go live" smoke test — it's documented for
operators, not automated by Unit 2.

---

## What this document does NOT cover

- **Terraform / CDK / Pulumi IaC** — Unit 2 documents the target state;
  provisioning automation is out of scope. A separate IaC repository or
  module can consume this document.
- **ECM → S3 sync pipeline** — how assets land in the bucket in the first
  place is upstream of Rendition; Unit 2 consumes the bucket as a read-only
  source.
- **CloudFront / CDN configuration** — upstream of Rendition; cache-control
  headers are set by Rendition's HTTP layer (Unit 4), but the CDN itself
  is out of scope.
- **Prometheus / OTEL exporter wiring** — Unit 7.
- **Cost monitoring alarms** — operational concern; CloudWatch costs are
  negligible for this unit's scope.
