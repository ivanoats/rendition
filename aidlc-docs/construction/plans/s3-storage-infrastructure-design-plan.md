# Unit 2 — S3 Storage Backend: Infrastructure Design Plan

**Unit:** S3 Storage Backend (Unit 2 of 7)
**Stage:** Infrastructure Design (Part 1 — Planning)
**Depth:** Standard
**Security extension:** Enabled — **SECURITY-01 (at rest) and SECURITY-06 (IAM)
deferred from NFR Requirements stage land here**

---

## Context Loaded

- `aidlc-docs/construction/s3-storage/functional-design/*`
- `aidlc-docs/construction/s3-storage/nfr-requirements/*`
- `aidlc-docs/construction/s3-storage/nfr-design/*`
- `aidlc-docs/inception/requirements/requirements.md` — QA-01, QA-02, QA-05
- `.aidlc-rule-details/extensions/security/baseline/security-baseline.md` —
  SECURITY-01 at-rest verification, SECURITY-06 least-privilege verification
- `docs/adr/0004-pluggable-storage-backends.md`,
  `docs/adr/0019-s3-circuit-breaker.md`,
  `docs/adr/0020-nested-config-groups.md`

## Scope

This stage maps Unit 2's logical components to **deployed infrastructure**:

- The S3 bucket itself — encryption, versioning, public-access block,
  lifecycle
- The IAM role/policy for Rendition pods to read the bucket
  (SECURITY-06 deferred here)
- Credential delivery mechanism (IRSA / instance profile / K8s secret)
- Network path to S3 (public AWS endpoint vs VPC gateway endpoint)
- LocalStack infrastructure for dev and CI environments
- Environment separation (dev / staging / prod)

**Non-goals:** bucket provisioning automation (Terraform/CDK) — this stage
documents the required state; the actual IaC can be authored separately
and is not a Unit 2 deliverable.

## Deliverables (Part 2 output)

- `aidlc-docs/construction/s3-storage/infrastructure-design/infrastructure-design.md`
- `aidlc-docs/construction/s3-storage/infrastructure-design/deployment-architecture.md`

## Plan Checklist

- [ ] Confirm scope with user
- [ ] Collect answers to `[Answer]:` questions
- [ ] Resolve ambiguities with follow-ups
- [ ] Verify SECURITY-01 (at-rest) compliance closes with answered choices
- [ ] Verify SECURITY-06 (least privilege) compliance closes with answered choices
- [ ] Generate `infrastructure-design.md` (bucket + IAM + network config)
- [ ] Generate `deployment-architecture.md` (environment topology + diagrams)
- [ ] Run markdownlint
- [ ] Present stage-completion message with security compliance summary
- [ ] Record approval in `audit.md`

---

## Clarification Questions

### Q1 — Target cloud & compute platform

The rest of the requirements assume AWS, but the deployment target for
Rendition hasn't been pinned anywhere I can see.

| Option | Compute | Credentials mechanism | Complexity |
|---|---|---|---|
| A. ⭐ AWS EKS (Kubernetes) with **IRSA** (IAM Roles for Service Accounts) | Kubernetes pods on EKS | OIDC trust policy + `sts:AssumeRoleWithWebIdentity` automatic via SDK | Medium |
| B. AWS ECS Fargate | ECS task on Fargate | Task role via ECS metadata endpoint (automatic) | Low |
| C. AWS EC2 Auto Scaling group | EC2 instances | Instance profile (automatic via IMDSv2) | Low |
| D. Generic Kubernetes (non-AWS) + static S3 credentials from K8s Secret | Any K8s | `AWS_ACCESS_KEY_ID`/`AWS_SECRET_ACCESS_KEY` env vars from Secret | **Higher risk** — long-lived creds to rotate |
| E. Multi-cloud (AWS + GCS + Azure Blob) | TBD | TBD | High — out of Unit 2 scope |

**Recommended: A.** EKS + IRSA is the modern AWS best practice: short-lived
credentials (automatic SDK refresh), no long-lived secrets to rotate, and
full compatibility with the default `aws-config` provider chain we chose in
NFR Req Q7=A. ECS (B) and EC2 (C) both work but EKS gives more operational
flexibility for a multi-service CDN. D is explicitly less secure (long-lived
credentials violate defense-in-depth) and should only be chosen when EKS/ECS
aren't available. E is a valid future direction but outside Unit 2's scope.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q2 — Bucket encryption at rest (SECURITY-01 deferred item)

SECURITY-01 requires "Encryption at rest enabled using a managed key service
or customer-managed keys". The bucket must have `BucketEncryption` configured.

| Option | Key management | Cost | Audit trail | Use case |
|---|---|---|---|---|
| A. ⭐ **SSE-S3** (AES-256, AWS-managed) | AWS-managed | Zero | S3 access logs only | Standard — sufficient for media assets that are not regulated PII |
| B. **SSE-KMS** (with default AWS-managed KMS key, `aws/s3`) | AWS-managed KMS key | ~$0.03 per 10k requests + tiny | CloudTrail shows key use | Preferred if you need per-request audit |
| C. **SSE-KMS** with customer-managed KMS key (CMK) in a dedicated key policy | Customer-managed | KMS key cost + per-request | Full CloudTrail + key access control | Regulated data, HIPAA/PCI environments |
| D. Bring-your-own key (SSE-C) | Customer-owned | Zero storage | No — AWS doesn't know the key | Very high-control scenarios; complicates SDK usage |

**Recommended: A — SSE-S3.** For a media CDN serving images and video to
anonymous end users, SSE-S3 meets SECURITY-01 (encrypted at rest,
AWS-managed) and the Rendition service never inspects keys anyway. SSE-KMS
(B) adds per-request cost at CDN scale (hundreds of thousands of GetObject
calls) with no user-visible benefit. C is worth reconsidering if Rendition
is ever extended to serve regulated content, but that decision belongs
downstream. D is hostile to the SDK's efficient multi-range support.

**Compliance note:** the choice will be enforced via a bucket policy **Deny**
statement that rejects `PutObject` without the expected encryption header.
This is the "rejects non-compliant writes via policy" language from
SECURITY-01's verification.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q3 — Public access block and bucket policy for anonymous reads

Rendition reads bytes from S3 and re-serves them through its own HTTP layer —
the bucket itself does **not** need public read access. End users never hit S3
directly.

| Option | Bucket exposure | CDN to S3 path |
|---|---|---|
| A. ⭐ **`BlockPublicAccess: ALL` enabled**, no public bucket policy. Rendition pods read via IAM role. | Private | Rendition pod → S3 (private) |
| B. Public-read bucket policy on specific prefixes (the "asset store") | Publicly listable/readable via URL | Rendition pod → S3 *or* end user → S3 directly |
| C. Presigned URLs — Rendition generates presigned GET URLs and the CDN fronts them | Private, time-scoped | CDN → presigned S3 URL |

**Recommended: A.** SECURITY-09 mandates "Cloud object storage MUST block
public access unless explicitly required and documented." Rendition's
architecture requires the bytes to flow through its transform pipeline, so
the bucket never needs to be public. `BlockPublicAccess: ALL` is the correct
stance. B breaks the embargo enforcement flow — if end users can hit S3
directly, Rendition can't gate them. C is useful for very-large files to
offload CDN egress, but adds complexity and doesn't fit the current pipeline.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q4 — Bucket versioning

S3 bucket versioning retains previous object versions on overwrite or delete.

| Option | Storage cost | Recovery on delete | Relevance to Rendition reads |
|---|---|---|---|
| A. **Enabled** | ~2× for frequently-overwritten buckets | Full (restore any version) | Rendition reads `null` version only unless callers opt in |
| B. ⭐ **Disabled** | Baseline | None via S3; must be rebuilt from ECM source of truth | Rendition doesn't care about versions |
| C. Enabled + lifecycle policy transitioning noncurrent versions to Glacier after 30 days | Slightly > baseline | Delayed (Glacier restore) | Operational recovery only |

**Recommended: B.** Rendition is a **read-only consumer** of the bucket. The
authoritative source of media is the ECM upstream of Rendition; the S3
bucket is effectively a cached view. Versioning adds cost and operational
surface area for a recovery path that's better served by re-syncing from
ECM. If a future requirement demands point-in-time recovery, moving to C
(versioning + lifecycle) is additive.

[Answer]: B (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q5 — IAM policy scope (SECURITY-06 deferred item)

SECURITY-06 requires least-privilege IAM with specific resource ARNs and
actions. What's the exact policy shape for Rendition's role?

| Option | Actions | Resources | Conditions |
|---|---|---|---|
| A. ⭐ `s3:GetObject`, `s3:GetObjectVersion` (read), `s3:ListBucket`, `s3:HeadObject` on `arn:aws:s3:::{bucket}` and `arn:aws:s3:::{bucket}/{prefix}*` | Narrow | Specific ARN + prefix | Condition: `aws:ResourceTag/Environment = {env}` to prevent cross-env access |
| B. Same actions, bucket-wide (no prefix scoping) | Narrow | Specific ARN bucket-wide | None |
| C. `s3:Get*` + `s3:List*` (wildcard actions) | Broad | Specific ARN | None |
| D. Terraform/CDK output — defer to an IaC module that we approve separately | Deferred | — | — |

**Recommended: A.** SECURITY-06's verification item says "No policy contains
wildcard actions or wildcard resources without a documented exception." A
gives the minimum four read actions on the narrowest ARN pattern. Prefix
scoping (`/{prefix}*`) means a single bucket can host multiple tenants with
per-tenant IAM roles, which is forward-compatible with multi-tenant
deployments. The tag condition is defense-in-depth against configuration
drift where a staging role gets attached to a prod cluster.

**Concrete policy JSON is included in the generated
`infrastructure-design.md`.**

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q6 — Network path to S3

AWS lets you reach S3 via the public regional endpoint (NAT gateway egress)
or via a VPC gateway endpoint (free, stays inside AWS network).

| Option | Cost | Latency | Security posture |
|---|---|---|---|
| A. ⭐ **VPC gateway endpoint** for S3 (`com.amazonaws.{region}.s3`), with endpoint policy restricting to the Rendition bucket ARN | Zero (gateway endpoints are free) | Lower (direct AWS backbone) | Stronger — traffic never leaves the VPC |
| B. Public S3 endpoint via NAT gateway | ~$0.045/GB egress through NAT | Slightly higher | Still encrypted (HTTPS), but traffic traverses public AWS network |
| C. VPC interface endpoint (`com.amazonaws.vpce.{region}.s3`) | ~$0.01/hour per endpoint + $0.01/GB | Lower | Strongest (interface endpoints support SG/NACL controls) |

**Recommended: A.** VPC **gateway** endpoints for S3 and DynamoDB are free —
zero cost to add, and they cut out the NAT gateway egress bill entirely for
S3 traffic. The endpoint policy restricts access to the Rendition bucket
ARN, which composes with Q5's IAM policy for defense in depth (SECURITY-11).
Interface endpoints (C) are the right choice for services that don't support
gateway endpoints — for S3 specifically, gateway is both cheaper and
equivalently secure.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q7 — Environment separation (dev / staging / prod)

How are the three typical environments isolated at the bucket level?

| Option | Blast radius | Cost | Operational simplicity |
|---|---|---|---|
| A. ⭐ **Separate buckets per environment**: `rendition-dev-assets`, `rendition-staging-assets`, `rendition-prod-assets`, each in its own AWS account | Minimal — account boundary | Minor (3 buckets, 3 accounts) | High — idiomatic AWS multi-account |
| B. Single bucket with environment prefix: `rendition-assets/{env}/*` | Medium — IAM must scope prefixes | Low | Medium — one bucket to manage |
| C. Separate buckets in the same AWS account | Medium — account shared | Minor | Medium |

**Recommended: A.** AWS Organizations + per-environment accounts is the
industry standard for blast-radius isolation. `RENDITION_S3_BUCKET` differs
per environment, so the config side of this is trivial. The prefix approach
(B) is cheaper to stand up but much riskier: a bug in IAM scoping leaks
prod data to staging. If Rendition is deployed into an existing multi-account
AWS Org, A is free; if not, this stage is the right time to request the
accounts be created.

**Fallback for small teams:** if multi-account is organisationally infeasible
right now, use C (separate buckets in one account) as a stepping stone, with
a documented migration to A.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q8 — Object lifecycle policy

Lifecycle rules can transition infrequently-accessed objects to cheaper
storage tiers and delete abandoned multipart uploads.

| Option | Rules |
|---|---|
| A. ⭐ **Minimal**: (1) Abort incomplete multipart uploads after 7 days. Nothing else. | Low-touch; Rendition serves frequently-accessed media |
| B. A + transition to **Standard-IA after 30 days** if noncurrent/rarely accessed | Moderate savings for long-tail content |
| C. A + B + transition to **Glacier Instant Retrieval after 180 days** | Max savings, slightly higher retrieval latency |
| D. No lifecycle policy | Noncompliant with AWS hardening baseline — abandoned uploads accrue indefinitely |

**Recommended: A.** Rendition serves images and video as a hot cache; cold
tier transitions would introduce unexpected retrieval latency for long-tail
assets. The "abort incomplete multipart after 7 days" rule is a free
hardening win that prevents runaway storage cost from broken uploaders. If a
future analytics exercise shows significant cold-tail savings, B/C can be
added without code changes.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q9 — LocalStack for dev environment (beyond tests)

Integration tests use LocalStack via `testcontainers-modules` (NFR Req Q5).
Is there also a use for LocalStack as a developer's local S3 substitute
during `cargo run`?

| Option | Dev loop | Dev environment setup | Realism |
|---|---|---|---|
| A. ⭐ **Only for tests** — dev `cargo run` uses `LocalStorage` by default (no S3 needed) | Fast | Zero | Low for dev; high for tests |
| B. Also for dev `cargo run` — developers start a LocalStack container via `docker compose up` and point `RENDITION_S3_ENDPOINT` at it | Slower startup | Requires Docker | Higher |
| C. Require a real AWS dev account for `cargo run` | Depends on network | Requires AWS creds | Highest |

**Recommended: A.** `LocalStorage` (Unit 1) is the right dev backend for
most daily development — it's zero-setup and faster than any alternative.
LocalStack is for verifying the S3 code path specifically, which is what
the integration test suite does. If a developer is actively working on
`s3.rs`, they can `cargo test -- --ignored` to exercise it; no need to run
the full server against LocalStack.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

### Q10 — Monitoring & alerting hooks from infrastructure (separate from Unit 7 app metrics)

Infrastructure-level signals from S3 that aren't in `StorageMetrics`:

| Option | Signals |
|---|---|
| A. ⭐ **CloudWatch metrics on the S3 bucket** (`NumberOfObjects`, `BucketSizeBytes`) + **VPC endpoint data processed** via CloudWatch. Alarms documented, provisioning deferred. | Basic infra visibility |
| B. A + **S3 Server Access Logs** to a dedicated log bucket, queried via Athena | Audit trail for compliance |
| C. A + B + **AWS GuardDuty** on the bucket | Threat detection |
| D. None — rely entirely on Rendition's app-level metrics (Unit 7) | Simplest |

**Recommended: A.** Free CloudWatch metrics on the bucket plus the VPC
endpoint give enough infra-side visibility without standing up a log
pipeline. S3 Server Access Logs (B) are valuable when compliance audits
require them, but for a CDN read path the volume is enormous and Athena
querying is a separate project. GuardDuty (C) is great but scope-creeps
Unit 2. D drops an entire signal channel — infra alerts fire before app
metrics do in a real outage.

**Note:** the alarms are *documented* in the infrastructure design — the
provisioning (Terraform/CDK) is not a Unit 2 deliverable.

[Answer]: A (take recommendation — AWS happy path; multi-cloud posture noted in artifact)

---

## Security compliance checkpoint

This stage is where SECURITY-01 (at rest) and SECURITY-06 (IAM) close.

| Rule | Verification target | Resolution in this stage |
|---|---|---|
| SECURITY-01 at rest | "Every data persistence store MUST have: encryption at rest enabled using a managed key service" | Q2=A → SSE-S3 at bucket level enforced via `BucketEncryption` + deny policy on non-encrypted `PutObject` |
| SECURITY-06 least privilege | "No policy contains wildcard actions or wildcard resources without a documented exception" | Q5=A → specific actions on specific ARNs with prefix scoping + tag conditions |
| SECURITY-07 restrictive network | "Private endpoints are used for high-traffic cloud service calls where available" | Q6=A → VPC gateway endpoint for S3 |
| SECURITY-09 hardening — public access blocked | "Cloud object storage has public access blocked" | Q3=A → `BlockPublicAccess: ALL` |
| SECURITY-09 hardening — no `latest` tags | Already resolved in NFR Req Q6=A (LocalStack `3.8`) | N/A here |

With the recommended answers, **zero blocking security findings remain**.
