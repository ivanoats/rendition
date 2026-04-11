# Unit 2 — S3 Storage Backend — Infrastructure Design

**Answers reference:** `aidlc-docs/construction/plans/s3-storage-infrastructure-design-plan.md`
**User directive:** "pluggable multi-cloud with happy path for AWS"

This document pins the **AWS happy path** in concrete detail and specifies
the **pluggable boundary** that keeps non-AWS deployments open for future
work.

---

## Multi-cloud posture

### Pluggability boundaries (three layers)

Rendition achieves multi-cloud readiness at three distinct layers. Only
**L2** requires code changes per cloud; **L1** and **L3** are pure
configuration.

**L1 — S3-compatible stores** (code-path reuse, configuration only)

The `S3Storage` backend works unchanged against any S3 API-compatible store
by setting `RENDITION_S3_ENDPOINT` and credentials:

| Store | Endpoint pattern | Tested in Unit 2? |
|---|---|---|
| **AWS S3** (happy path) | Default regional | Yes — real service + LocalStack |
| **LocalStack** | `http://localhost:4566` (test only, `allow_insecure_endpoint`) | Yes |
| **Cloudflare R2** | `https://{account}.r2.cloudflarestorage.com` | No — expected to work; not verified in Unit 2 |
| **MinIO** | `https://minio.example.com` | No — expected to work |
| **Wasabi / DigitalOcean Spaces / Backblaze B2** | per-provider endpoint | No — expected to work |

No code changes are needed for L1 stores. A future "compatibility test suite"
(deferred from NFR Req Q11=A) can verify these empirically.

**L2 — Non-S3 object stores** (new `StorageBackend` implementation)

Google Cloud Storage and Azure Blob Storage are **not** S3-API-compatible
at the protocol level (they have compatibility shims but subtle semantic
differences — conditional headers, range syntax, error codes). Supporting
them cleanly requires a new `StorageBackend` impl per cloud:

- `src/storage/gcs.rs` — `GcsStorage` using `google-cloud-storage` crate
- `src/storage/azure.rs` — `AzureBlobStorage` using `azure_storage_blobs` crate

Each new impl:

- Satisfies the same `StorageBackend` trait and `StorageError` contract
- Owns its own cloud-specific client, credentials chain, and error
  classification (R-01)
- Can reuse `CircuitBreaker` unchanged — the breaker is cloud-agnostic
- Adds a new variant to `StorageBackendKind` in `AppConfig`

Unit 2 does **not** implement L2 — it is future work. The architecture
(ADR-0004's trait abstraction) does not preclude it.

**L3 — Cloud-specific infrastructure** (per-cloud ops decisions)

IAM, networking, monitoring, and encryption are cloud-specific concerns. The
AWS happy path is documented in detail below. Equivalents for GCP and Azure
are sketched in the "Non-AWS operator runbook" section at the end — not
detailed, but the patterns translate.

---

## AWS happy path — detailed design

### A1 — S3 bucket configuration

**Bucket naming:** `rendition-{env}-assets` where `{env}` ∈
`{dev, staging, prod}`. Three separate buckets, each in its own AWS account
(Q7=A).

**Core settings:**

| Setting | Value | Rule addressed |
|---|---|---|
| `BucketEncryption` | `AES256` (SSE-S3, Q2=A) | SECURITY-01 at-rest |
| `PublicAccessBlock` | all four flags `true` (Q3=A) | SECURITY-09 |
| `BucketVersioning` | Suspended (Q4=B) | Cost / operational simplicity |
| `ObjectOwnership` | `BucketOwnerEnforced` (ACLs disabled) | SECURITY-09 hardening |
| `LifecycleConfiguration` | Abort incomplete multipart uploads after 7 days (Q8=A) | SECURITY-09 |
| Region | `us-west-2` (prod/staging); any for dev | QA-01 locality |

**Enforcement policy — reject unencrypted writes** (SECURITY-01 verification):

```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Sid": "DenyUnencryptedPutObject",
    "Effect": "Deny",
    "Principal": "*",
    "Action": "s3:PutObject",
    "Resource": "arn:aws:s3:::rendition-{env}-assets/*",
    "Condition": {
      "StringNotEquals": {
        "s3:x-amz-server-side-encryption": "AES256"
      }
    }
  }]
}
```

**Why `BucketOwnerEnforced`:** disables all ACLs, forcing IAM-only access
control. Eliminates a whole class of misconfiguration (object-owner mismatch,
legacy public-read ACLs). AWS best practice since 2023.

### A2 — IAM role for Rendition (Q5=A)

**Role name:** `rendition-{env}-s3-reader`
**Trust policy:** IRSA (Q1=A) — trusts the EKS OIDC provider for the
`rendition` service account in the `rendition` namespace:

```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": {
      "Federated": "arn:aws:iam::{account}:oidc-provider/oidc.eks.{region}.amazonaws.com/id/{cluster-id}"
    },
    "Action": "sts:AssumeRoleWithWebIdentity",
    "Condition": {
      "StringEquals": {
        "oidc.eks.{region}.amazonaws.com/id/{cluster-id}:sub":
          "system:serviceaccount:rendition:rendition",
        "oidc.eks.{region}.amazonaws.com/id/{cluster-id}:aud":
          "sts.amazonaws.com"
      }
    }
  }]
}
```

**Permissions policy** — exact actions, specific ARNs, tag condition:

```json
{
  "Version": "2012-10-17",
  "Statement": [
    {
      "Sid": "ReadObjects",
      "Effect": "Allow",
      "Action": [
        "s3:GetObject",
        "s3:GetObjectVersion"
      ],
      "Resource": "arn:aws:s3:::rendition-{env}-assets/*",
      "Condition": {
        "StringEquals": {
          "aws:ResourceTag/Environment": "{env}"
        }
      }
    },
    {
      "Sid": "HeadAndList",
      "Effect": "Allow",
      "Action": [
        "s3:HeadObject",
        "s3:ListBucket"
      ],
      "Resource": "arn:aws:s3:::rendition-{env}-assets",
      "Condition": {
        "StringEquals": {
          "aws:ResourceTag/Environment": "{env}"
        }
      }
    }
  ]
}
```

**Verification against SECURITY-06:**

- ✅ No wildcard actions (`s3:*` forbidden)
- ✅ No wildcard resources (`"*"` forbidden)
- ✅ Actions split between read-object and list/head (SECURITY-06 preference)
- ✅ Tag condition prevents cross-environment drift

### A3 — Network path (Q6=A)

**VPC gateway endpoint for S3:**

- Endpoint type: `Gateway`
- Service: `com.amazonaws.{region}.s3`
- Route tables: attached to the private subnets where Rendition pods run
- Endpoint policy (scoped to the Rendition bucket ARN — defense in depth
  layered with Q5's IAM):

```json
{
  "Version": "2012-10-17",
  "Statement": [{
    "Effect": "Allow",
    "Principal": "*",
    "Action": [
      "s3:GetObject",
      "s3:GetObjectVersion",
      "s3:HeadObject",
      "s3:ListBucket"
    ],
    "Resource": [
      "arn:aws:s3:::rendition-{env}-assets",
      "arn:aws:s3:::rendition-{env}-assets/*"
    ]
  }]
}
```

**Traffic flow:** Rendition pod → EKS node ENI (private subnet) →
gateway endpoint → S3. No NAT gateway hop, no public internet, no egress
cost for S3 traffic.

### A4 — Monitoring (Q10=A)

Documented for future provisioning — not part of Unit 2's shipped code.

**CloudWatch metric alarms on the bucket:**

| Alarm | Metric | Threshold | Action |
|---|---|---|---|
| `RenditionBucket-HighErrorRate` | `5xxErrors` via S3 server-side metrics | > 5 / minute for 5 minutes | SNS → on-call |
| `RenditionBucket-HighLatency` | `FirstByteLatency` P99 | > 500 ms for 5 minutes | SNS → on-call |
| `RenditionBucket-ObjectCountDrop` | `NumberOfObjects` delta | Sudden drop > 10% | SNS → on-call (possible ECM outage) |

**VPC endpoint CloudWatch:**

| Metric | Purpose |
|---|---|
| `BytesProcessed` | Correlate to app-level request volume |
| `PacketDropCount` | Endpoint saturation |

**Alarms are documented, not provisioned in Unit 2.** The consuming ops
team (or the Infrastructure-as-Code team) can render these into Terraform
or CDK.

---

## Rendition configuration for the AWS happy path

The following env vars are set in the EKS pod `Deployment` for the
`rendition` workload:

```yaml
env:
  - name: RENDITION_STORAGE_BACKEND
    value: "s3"
  - name: RENDITION_S3_BUCKET
    value: "rendition-prod-assets"
  - name: RENDITION_S3_REGION
    value: "us-west-2"
  - name: RENDITION_S3_PREFIX
    value: "v1/"
  - name: RENDITION_S3_MAX_CONNECTIONS
    value: "100"
  - name: RENDITION_S3_TIMEOUT_MS
    value: "5000"
  - name: RENDITION_S3_CB_THRESHOLD
    value: "5"
  - name: RENDITION_S3_CB_COOLDOWN_SECONDS
    value: "30"
  - name: RENDITION_S3_MAX_RETRIES
    value: "3"
  - name: RENDITION_S3_RETRY_BASE_MS
    value: "50"
  # RENDITION_S3_ENDPOINT is intentionally unset — the SDK resolves the
  # AWS regional endpoint automatically from s3_region.
  # RENDITION_S3_ALLOW_INSECURE_ENDPOINT is also unset (default false).
serviceAccountName: rendition   # annotated with eks.amazonaws.com/role-arn
```

**No AWS credentials environment variables.** IRSA gives the pod
short-lived credentials automatically via the SDK's default credential chain
(NFR Req Q7=A).

---

## Non-AWS operator runbook (sketch, not shipped in Unit 2)

### GCP target

| AWS concept | GCP equivalent |
|---|---|
| S3 bucket | GCS bucket |
| IRSA | Workload Identity |
| Bucket encryption SSE-S3 | Google-managed encryption key (default) |
| Bucket policy `BlockPublicAccess` | Bucket IAM with no `allUsers` binding + `uniform bucket-level access` |
| IAM policy `s3:GetObject` | `roles/storage.objectViewer` scoped to bucket resource name |
| VPC gateway endpoint | Private Google Access |
| CloudWatch metrics | Cloud Monitoring bucket metrics |

**Code impact:** new `GcsStorage` implementing `StorageBackend`, using the
`google-cloud-storage` Rust crate (L2 from the pluggability section).
Deferred.

### Azure target

| AWS concept | Azure equivalent |
|---|---|
| S3 bucket | Azure Blob container |
| IRSA | Azure AD Workload Identity (preview) or pod-identity |
| Bucket encryption SSE-S3 | Microsoft-managed keys (default) |
| BlockPublicAccess | Container `privateAccess` setting + network rules |
| IAM policy | RBAC `Storage Blob Data Reader` role scoped to container |
| VPC endpoint | Private Endpoint for Blob Storage |
| CloudWatch | Azure Monitor |

**Code impact:** new `AzureBlobStorage` implementing `StorageBackend` using
the `azure_storage_blobs` crate. Deferred.

### S3-compatible (Cloudflare R2, MinIO, Wasabi, DO Spaces, Backblaze B2)

No code impact — already supported by `S3Storage` via
`RENDITION_S3_ENDPOINT`. Per-provider notes:

- **R2:** requires `force_path_style` mode. The AWS SDK honours this via
  `aws_config::Builder::force_path_style(true)`. Unit 2 does not flip this
  flag automatically; operators set it via SDK config override or a future
  `RENDITION_S3_FORCE_PATH_STYLE` env var.
- **MinIO:** HTTPS endpoint needs a public cert or the `allow_insecure_endpoint`
  flag; same escape hatch as LocalStack.
- **Wasabi / B2 / DO Spaces:** all tested via the S3 API by other Rust
  projects; expected to work out of the box.

---

## Security compliance closure

All security items deferred from NFR Requirements are resolved in this
stage:

| Rule | Resolution |
|---|---|
| SECURITY-01 in-transit | Already resolved in NFR Req Q8 (config validation rejects `http://` unless escape hatch) |
| **SECURITY-01 at-rest** | **Resolved here, Q2=A.** `BucketEncryption: AES256` + deny policy on non-encrypted `PutObject`. |
| SECURITY-02 access logging (LB/gateway/CDN) | N/A — no network intermediary at this layer; CDN is upstream of Rendition |
| SECURITY-03 structured app logging | Resolved in NFR Req (tracing spans) |
| SECURITY-05 input validation | Resolved in Functional Design (compose_key, range invariants) |
| **SECURITY-06 least privilege** | **Resolved here, Q5=A.** Narrow actions, specific ARN with prefix, tag condition. |
| SECURITY-07 restrictive network | **Resolved here, Q6=A.** VPC gateway endpoint with endpoint policy. Private subnets with NAT for other egress. |
| SECURITY-08 app access control | N/A — public CDN reads; admin auth in Unit 5 |
| **SECURITY-09 public access block** | **Resolved here, Q3=A.** `BlockPublicAccess: ALL` enabled. Also Q8=A (lifecycle hardening) and `BucketOwnerEnforced`. |
| SECURITY-10 supply chain | Resolved in NFR Req (pinned LocalStack, cargo-audit in CI) |
| SECURITY-11 defense in depth / rate limiting | Partial — circuit breaker (this unit), rate limiting deferred to Unit 6 |
| SECURITY-12 authentication | N/A — no user auth in this unit |
| SECURITY-13 deserialization | Resolved in NFR Req (raw bytes) |

**Zero blocking security findings.**
