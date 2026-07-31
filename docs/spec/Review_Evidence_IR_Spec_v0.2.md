# Review Evidence IR (REIR) Specification v0.2

Status: Implementation-aligned / cross-layer review evidence core
Version: 0.2
Audience: REIR implementers, adapter authors, RSScript compiler/package-tooling authors, infrastructure/IaC tool authors, CI/review-platform authors, IDE authors, AI review-agent authors, security and platform engineering teams
Scope: language-neutral review evidence model; semantic facts; evidence; confidence, acquisition, and precision; subject identity chains; capability ontology; required/granted/observed capability reconciliation; flow edges; slices; semantic diffs; exceptions; adapter profiles for RSScript, RSScript package metadata, K8s/IaC/IAM, runtime observation, and future systems-language producers
Non-scope: RSScript language semantics, RSScript package dependency resolution, Kubernetes semantics, Terraform semantics, cloud-provider IAM semantics, executable workflow DSLs, full runtime sandbox implementation, formal business-specification language, arbitrary program verification, replacement for SBOM/SLSA/provenance standards

### Changes from v0.1

```text
- RSScript adapter implemented and integrated into package review pipeline.
- Capability-binding call-graph propagation now produces REIR required-capability
  facts for each reachable API path.
- S3 IAM scenario demonstrates end-to-end: code requires → IAM grants →
  reconciliation result → review summary.
- Evidence bundle format includes reconciliation result (pass/fail/partial)
  with per-capability detail.
- Adapter profile for RSScript package metadata now emits structured
  capability-binding facts rather than heuristic text annotations.
- Unknown-reason field added to capability facts for incomplete analysis paths.
```

---

## 0. Scope and Boundary

REIR is a language-neutral evidence IR for reviewing large heterogeneous
systems. It is not a source language, not an application framework, and not a
runtime. It is the common evidence layer into which source code, package
metadata, dependency graphs, infrastructure configuration, deployment identity,
cloud authorization, and runtime observations can emit review facts.

REIR exists to change the review unit:

```text
not raw source code
not raw YAML
not raw Terraform
not raw telemetry
but semantic fact with evidence, confidence, acquisition mode, precision, and diff identity
```

The central purpose of REIR is cross-layer review. Single-layer tools can already
find many facts inside source, IaC, dependency metadata, or runtime logs. REIR's
non-replaceable value is connecting facts that belong to different layers and
checking whether the combined path is safe, available, consistent, and changed.

The first product demonstration is **cross-layer capability reconciliation**:

```text
code requires a capability
  + deployment identity receives or lacks that capability
  + runtime may or may not observe that capability
  => missing / excess / unexpected / unused capability result before or after deploy
```

Example:

```text
code now requires aws.s3.PutObject on reports-prod/exports/*
workload runs as Kubernetes ServiceAccount checkout-api
service account maps to AWS IAM role checkout-prod
role does not grant s3:PutObject on that resource

result: missing capability; this deployment is likely to fail at runtime
```

### 0.1 Review evidence, not formal proof

REIR is the review evidence layer. It is not an SMT system, proof assistant, or
complete verifier for arbitrary program behavior.

Formal verification is valuable when the target property is precise, local, and
stable enough to model. Review has a different scaling problem. A real PR can
change source code, generated code, package metadata, native wrappers,
deployment identity, IAM, runtime configuration, and observed behavior at the
same time. The reviewer usually needs to answer a throughput question:

```text
what new semantic facts changed,
which facts are backed by source or producer-owned semantics,
which facts are best-effort or unknown,
and which changed facts cross a deployment or capability boundary?
```

REIR optimizes for that review path. It records evidence, acquisition mode,
confidence, precision, subject identity, and diff identity so review can move
from O(total artifact volume) toward O(changed semantic facts). A producer may
emit authoritative facts only for semantics it owns. Heuristic scans,
AI-inferred facts, incomplete analysis, and runtime observations must remain
marked as such rather than being upgraded into proof.

### 0.2 Normative boundary

REIR owns the shape of review evidence. It does **not** own the semantics of any
producer.

```text
RSScript owns RSScript language semantics.
RSScript package tooling owns RSScript package, dependency, lockfile, and graph semantics.
Kubernetes owns Kubernetes desired-state semantics.
Terraform and cloud providers own their plan, resource, and authorization semantics.
Runtime and audit systems own observed events according to their collection scope.
REIR owns how those facts are represented, related, compared, sliced, and reconciled.
```

A REIR producer must not claim stronger authority than its source supports. A
source-derived fact can be authoritative only for the source semantics it owns.
A scanner-derived or AI-inferred fact must remain lower-confidence and must carry
its acquisition mode.

### 0.3 Compatibility with RSScript v0.5 specs

This specification is compatible with the RSScript language and package-manager
specifications by treating them as producer specifications, not as sub-sections
of REIR.

```text
RSScript language specification:
  defines language semantics and can emit authoritative language-level REIR facts
  for read/mut/take, retains, resources, fresh, local/manage, native/unsafe,
  static protocol contracts, native module normalized declarations, diagnostics,
  review-map classifications, and source-backed evidence.

RSScript package-manager specification:
  defines package/dependency semantics and can emit package-level and graph-level
  REIR facts for .rssi contracts, effective interfaces, dependency paths,
  direct dependency identities, package risk, native wrapper facts, build-time
  execution facts, capability metadata, lockfile evidence, and graph review
  summaries.

REIR:
  consumes those facts and can relate them to infrastructure, identity,
  authorization, runtime, and cross-layer deployment facts.
```

REIR does not add RSScript syntax, RSScript effects, RSScript type rules,
package resolution rules, `.rssi` normalization rules, or native wrapper
conformance rules.

### 0.4 Design thesis

Software generation cost is collapsing. Review cost does not collapse with it.
The limiting question is no longer whether a human or AI can read enough raw
artifacts. The limiting question is whether system behavior can be stabilized as
reviewable evidence and whether review can be reduced from O(total artifact
volume) to O(changed semantic facts).

REIR therefore has four design goals:

```text
1. Turn heterogeneous system behavior claims into evidence-backed facts.
2. Connect facts across source, packages, infrastructure, identity, authorization, and runtime.
3. Compare required, granted, and observed behavior using one capability vocabulary.
4. Make review diff-first and slice-first, not full-graph-first.
```

### 0.4 Core principles

```text
1. Review facts, not raw artifacts.
2. Every fact must have evidence.
3. Every evidence item must be traceable.
4. Every unknown must have a reason.
5. Every fact must carry confidence, acquisition mode, and precision.
6. Required, granted, and observed behavior are different fact roles.
7. Missing capability is an availability risk; excess capability is a security risk.
8. Dependencies, build-time behavior, deployment identity, and runtime observation are system behavior.
9. Flow is generated, descriptive, evidence-backed, and non-executable.
10. Review slices, not whole graphs.
11. Review semantic diffs, not full baselines.
12. Runtime enforcement is the final enforcement layer, not the first discovery layer.
13. AI should reason over generated evidence, not rediscover facts from raw artifacts.
```

### 0.5 Non-goals

REIR v0.2 does not attempt to provide:

```text
an executable workflow language
a policy language replacing OPA, Cedar, IAM, Kubernetes RBAC, or cloud authorization
an AST/HIR/MIR/bytecode representation
a full static analyzer for arbitrary programming languages
a sandbox implementation
a formal proof of business correctness
a universal cloud IAM evaluator
a replacement for SBOM, SLSA, provenance, vulnerability, or license standards
a guarantee that runtime observations are complete
a guarantee that declared desired state equals actual deployed state
```

REIR may reference SBOM, provenance, vulnerability, policy, or runtime systems as
evidence sources. It does not replace their native formats or authorities.

---

## 1. REIR Core Model

REIR v0.2 has a small object model:

```text
Subject             the thing a fact or edge is about
SubjectChain        a cross-layer identity chain for an execution subject
Capability          a normalized behavior/permission/action vocabulary item
Fact                an assertion about a subject
Evidence            traceable support for a fact or edge
Confidence          reliability of a fact
AcquisitionMode     how a fact was obtained
Precision           how specific a fact is
Edge                a relation between subjects or facts
Slice               a review-focused subgraph/fact set
PolicyResult        result of evaluating facts against policy/profile
Reconciliation      comparison between required, granted, and observed facts
Diff                added/removed/changed facts, edges, profiles, or results
Exception           accepted review debt with owner, reason, evidence, and expiry
```

REIR objects are machine-readable. Human reports, dashboards, and AI review packs
are views over REIR, not separate truth sources.

### 1.1 Required object fields

Every standalone REIR record should include the fields that identify its schema,
identity, and kind. For deterministic bundle artifacts, producer identity is
carried by the enclosing bundle's `producers` array rather than duplicated on
every embedded fact or edge.

```text
schema          schema identifier and version
id              stable object identity for records that are diffed by identity
kind            object kind for typed records such as facts, edges, slices, and diff items
producer        producer identity and version for standalone records, or bundle-level producers
created_at      generation timestamp, omitted in deterministic baselines and lockable bundles
```

Every `Fact` and `Edge` must include:

```text
subject or endpoints
evidence[]
confidence
acquisition_mode
precision
```

A fact with no evidence is invalid. An unknown fact without an acquisition reason
is invalid.

### 1.2 Producer identity

Producer identity records where the object came from:

```json
{
  "name": "rssc",
  "version": "0.5.0-alpha",
  "adapter": "rsscript-language",
  "adapter_version": "0.1",
  "source": "compiler_contract"
}
```

Examples of producer adapters:

```text
rsscript-language
rsscript-package
k8s-rendered-manifest
terraform-plan
cloud-iam-policy
runtime-cloudtrail
runtime-k8s-audit
source-scan-rust
source-scan-typescript
zig-system-contract
ai-inference
manual-declaration
```

### 1.3 Stable IDs and diff identity

REIR is diff-first. Producers must make IDs stable across runs when the
underlying object is unchanged.

Recommended ID sources:

```text
RSScript function       package + fully qualified symbol
RSScript interface      package + normalized .rssi symbol + effective interface hash
Package                 package name + version + source identity + selected features
Dependency edge          parent package identity + child package identity + selected features
K8s object               cluster/env + apiVersion + kind + namespace + name
Terraform resource       workspace + module path + resource address
IAM role/policy          provider + account/project + ARN/name + version/hash
Container image          registry + repository + digest
Runtime observation      provider event id or normalized event hash + time window
Capability fact          subject id + fact role + normalized capability key + scope
```

When stable identity is impossible, the producer must mark the fact as less
stable and include enough evidence for diff tools to compute best-effort matching.

---

## 2. Subject and SubjectChain Model

A `Subject` identifies the object being described.

Minimal subject:

```json
{
  "kind": "code.function",
  "id": "my-app::ReportUploader.upload",
  "name": "ReportUploader.upload",
  "package": "my-app"
}
```

### 2.1 Subject kinds

REIR v0.2 recognizes these subject families:

```text
application
service
entrypoint
code.file
code.region
code.module
code.function
code.public_api
code.interface_symbol
code.type
code.protocol
code.protocol_method
code.protocol_impl
package
package.version
package.feature
dependency
dependency.edge
build.artifact
build.step
container.image
k8s.cluster
k8s.namespace
k8s.resource
k8s.deployment
k8s.pod_template
k8s.service
k8s.ingress
k8s.service_account
k8s.secret
k8s.configmap
k8s.volume
terraform.workspace
terraform.resource
cloud.account
cloud.project
cloud.identity
cloud.role
cloud.policy
cloud.permission
runtime.process
runtime.workload
runtime.event
capability
native.boundary
native.module
unsafe.boundary
resource
profile
policy
exception
unknown
```

Adapters may emit extension subject kinds using reverse-DNS or tool-qualified
names, but core review engines are required to understand only the v0.2 core
families.

### 2.2 SubjectChain

A `SubjectChain` connects identities across layers. It is the core asset for
cross-layer review.

Example chain:

```json
{
  "schema": "reir.subject_chain.v0.2",
  "id": "chain.checkout.report_uploader.prod",
  "kind": "execution_subject_chain",
  "nodes": [
    { "kind": "code.function", "id": "checkout::ReportUploader.upload" },
    { "kind": "package", "id": "checkout@1.4.2" },
    { "kind": "container.image", "id": "registry.example.com/checkout@sha256:abc..." },
    { "kind": "k8s.deployment", "id": "prod/apps/Deployment/checkout-api" },
    { "kind": "k8s.service_account", "id": "prod/ServiceAccount/checkout-api" },
    { "kind": "cloud.role", "id": "aws:arn:aws:iam::123456789012:role/checkout-prod" }
  ],
  "edges": [
    { "kind": "built_as", "from": 0, "to": 2 },
    { "kind": "deployed_as", "from": 2, "to": 3 },
    { "kind": "runs_as", "from": 3, "to": 4 },
    { "kind": "assumes_role", "from": 4, "to": 5 }
  ],
  "evidence": [
    { "kind": "image_digest", "value": "sha256:abc..." },
    { "kind": "manifest_pointer", "file": "rendered/prod/deployment.yaml", "json_pointer": "/spec/template/spec/serviceAccountName" },
    { "kind": "manifest_pointer", "file": "terraform/prod/iam.tf", "json_pointer": "/resource/aws_iam_role/checkout_prod" }
  ]
}
```

The chain is not required to be complete. A missing chain segment is itself a
review fact:

```text
subject_chain_incomplete
  acquisition: identity_mapping_missing
  confidence: unknown
```

### 2.3 Chain edge kinds

Core chain edges:

```text
built_from
built_as
packaged_as
published_as
deployed_as
runs_as
routes_to
mounts
reads_secret
uses_service_account
assumes_role
bound_to_role
grants_permission
observed_as
unknown_link
```

Every chain edge must carry evidence. A chain edge inferred only from naming
convention must be marked `inferred` and must not be treated as authoritative.

### 2.4 Cross-repo truth

REIR assumes the subject chain may span multiple repositories:

```text
application source repo
container build repo
Helm chart repo
Kustomize overlay repo
Terraform/IAM repo
runtime observation system
```

A REIR implementation may assemble chain fragments produced by different repos.
The resulting chain should record fragment provenance so a reviewer can tell
which repo supplied each link.

---

## 3. Fact Model

A `Fact` is a review-relevant assertion about a subject.

Minimal fact:

```json
{
  "schema": "reir.fact.v0.2",
  "id": "fact.checkout.report_uploader.requires.s3_put_object",
  "kind": "capability",
  "role": "required",
  "subject": { "kind": "code.function", "id": "checkout::ReportUploader.upload" },
  "capability": {
    "category": "object_storage.write",
    "provider": "aws",
    "action": "s3:PutObject",
    "resource": "arn:aws:s3:::reports-prod/exports/*"
  },
  "value": true,
  "confidence": { "level": "computed", "source": "sdk_mapping" },
  "acquisition_mode": "source_scan",
  "precision": "resource_scoped",
  "evidence": [
    { "kind": "source_span", "file": "src/report_upload.rs", "line": 88, "symbol": "s3.put_object" }
  ]
}
```

### 3.1 Fact roles

Capability-like facts use a `role` field:

```text
required    code, package, build step, or workload needs this capability
granted     identity, role, policy, deployment, or runtime environment grants this capability
observed    runtime/audit telemetry observed this capability being exercised
denied      policy/profile explicitly denies this capability
allowed     policy/profile explicitly allows this capability
expected    profile or declaration expects this capability to exist
unknown     a required/granted/observed status could not be determined
```

The role matters. A `required` S3 write fact and a `granted` S3 write fact do not
mean the same thing; reconciliation compares them.

### 3.2 Core fact kinds

```text
capability
resource
retention
mutation
native_boundary
unsafe_boundary
protocol_declaration
protocol_method_contract
protocol_impl
protocol_static_call
async_boundary
diagnostic
module_declaration
use_declaration
native_module_declaration
public_contract
package_feature
native_cargo_feature
provider_implementation
package_risk
dependency_risk
build_time_execution
supply_chain
identity
authorization
network_exposure
secret_access
storage_access
runtime_observation
profile_rule
policy_result
subject_chain
flow
unknown
```

Adapters may emit extension kinds. Extension kinds must still use the core
evidence, confidence, acquisition, and precision fields.

### 3.3 Unknown facts

Unknown facts are first-class. They must not be silently omitted.

Example:

```json
{
  "schema": "reir.fact.v0.2",
  "id": "fact.checkout.role.grants.s3_put_object.unknown",
  "kind": "capability",
  "role": "granted",
  "value": "unknown",
  "subject": { "kind": "cloud.role", "id": "aws:role/checkout-prod" },
  "capability": {
    "provider": "aws",
    "action": "s3:PutObject",
    "resource": "arn:aws:s3:::reports-prod/exports/*"
  },
  "confidence": { "level": "unknown", "source": "metadata_unavailable" },
  "acquisition_mode": "unknown",
  "precision": "resource_scoped",
  "unknown_reason": "iam_policy_document_not_available_for_target_environment",
  "evidence": [
    { "kind": "unknown_reason", "reason": "iam_policy_document_not_available_for_target_environment" }
  ]
}
```

### 3.4 Fact truth values

The `value` field may be:

```text
true
false
unknown
not_run
partial
```

`partial` means some part of the fact is known but not enough to decide full
coverage. For example, an IAM role may grant `s3:PutObject` for one bucket prefix
but the required resource expression cannot be resolved fully.

---

## 4. Evidence Model

Evidence records why a fact or edge exists.

### 4.1 Evidence hard rules

```text
1. Source-derived evidence must include a source span or symbolic pointer.
2. Manifest-derived evidence must include a file and JSON/YAML pointer where possible.
3. Plan-derived evidence must include a plan resource address or JSON pointer.
4. Policy-derived evidence must include a policy document pointer or provider identity.
5. Runtime-derived evidence must include event identity, timestamp/window, and collection source.
6. Unknown evidence must include a reason and acquisition context.
```

### 4.2 Evidence kinds

```text
source_span
interface_span
manifest_pointer
source_template_pointer
rendered_manifest_pointer
terraform_plan_pointer
terraform_state_pointer
cloud_policy_pointer
lockfile_entry
dependency_path
binding_manifest
cargo_metadata
package_metadata
registry_metadata
image_digest
provenance_attestation
sbom_entry
runtime_event
cloud_audit_event
k8s_audit_event
service_mesh_trace
ebpf_observation
sandbox_log
ai_inference_trace
manual_attestation
unknown_reason
```

### 4.3 Evidence examples

Source span:

```json
{
  "kind": "source_span",
  "file": "src/report_upload.rss",
  "line": 42,
  "column": 16,
  "length": 24,
  "symbol": "S3.put_object",
  "reason": "direct call to capability-providing API"
}
```

Rendered manifest pointer:

```json
{
  "kind": "rendered_manifest_pointer",
  "file": "rendered/prod/deployment.yaml",
  "json_pointer": "/spec/template/spec/serviceAccountName",
  "resource": "Deployment/prod/checkout-api"
}
```

Cloud policy pointer:

```json
{
  "kind": "cloud_policy_pointer",
  "provider": "aws",
  "account": "123456789012",
  "policy_arn": "arn:aws:iam::123456789012:policy/checkout-prod",
  "statement_index": 2,
  "action": "s3:PutObject",
  "resource": "arn:aws:s3:::reports-prod/exports/*"
}
```

Runtime observation:

```json
{
  "kind": "cloud_audit_event",
  "provider": "aws",
  "source": "cloudtrail",
  "event_id": "8e9f...",
  "time": "2026-05-30T10:22:11Z",
  "event_name": "PutObject",
  "principal": "arn:aws:sts::123456789012:assumed-role/checkout-prod/..."
}
```

### 4.4 Evidence aggregation

A fact may have multiple evidence items. Evidence items may support different
parts of a fact:

```text
source evidence       supports requirement
subject-chain evidence supports identity mapping
policy evidence       supports grant
runtime evidence      supports observed behavior
```

A review UI should show the smallest useful evidence first and allow expansion.

---

## 5. Confidence, Acquisition Mode, and Precision

### 5.1 Confidence levels

```text
authoritative    produced by the semantic authority for that layer
computed         derived deterministically from authoritative or structured input
declared         stated by author/package/profile, not independently verified
scanned          discovered by static or metadata scan
inferred         inferred by heuristic or AI from raw artifacts
observed         observed at runtime within a stated collection window
partial          some evidence exists but coverage is incomplete
unknown          fact could not be determined
not_run          check capable of producing the fact was not run
```

Examples:

```text
RSScript compiler emits retains(x) fact               authoritative
K8s rendered manifest says serviceAccountName         authoritative for desired state
Terraform plan says role policy will be attached      authoritative for planned state
IAM analyzer computes action coverage                 computed
Package author declares may_block=true                declared
Rust source scan finds std::fs::write                 scanned
AI says function appears to upload to S3               inferred
CloudTrail observed PutObject                         observed
Native unsafe scan was not run                        not_run
```

### 5.2 Acquisition modes

```text
compiler_contract
normalized_interface
package_metadata
lockfile
binding_manifest
rendered_manifest
terraform_plan
terraform_state
cloud_policy
cloud_authorization_analysis
runtime_observation
source_scan
binary_scan
provenance_attestation
sbom
manual_declaration
manual_exception
ai_inference
unknown
not_run
```

### 5.3 Precision levels

Precision describes how specific the fact is:

```text
exact              exact action and exact resource are known
resource_scoped    action known and resource scope known with provider semantics
symbolic           resource expression is known but not fully resolved
category           coarse capability category only
presence           only presence/absence of a class of behavior is known
unknown            precision cannot be determined
```

Examples:

```text
aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*      resource_scoped
object_storage.write to bucket variable `bucket_name`         symbolic
network.client                                                category
uses native code                                              presence
```

### 5.4 Confidence is not precision

A fact can be high confidence but low precision:

```text
rendered manifest authoritatively says a container has egress allowed,
but the destination is any IP address.
```

A fact can also be lower confidence but high precision:

```text
source scanner infers s3:PutObject on a concrete bucket from SDK call arguments.
```

Policy engines must consider both.

---

## 6. Capability Ontology

A capability is a normalized description of behavior, permission, or external
effect. Capabilities are used for cross-layer reconciliation.

### 6.1 Capability object

```json
{
  "category": "object_storage.write",
  "provider": "aws",
  "service": "s3",
  "action": "s3:PutObject",
  "resource": "arn:aws:s3:::reports-prod/exports/*",
  "constraints": {
    "region": "us-west-2"
  }
}
```

Required fields:

```text
category
```

Optional fields:

```text
provider
service
action
resource
constraints
```

### 6.2 Capability categories

Core categories:

```text
network.client
network.server
network.public_ingress
network.egress
filesystem.read
filesystem.write
object_storage.read
object_storage.write
object_storage.delete
database.read
database.write
queue.publish
queue.consume
secret.read
secret.write
env.read
env.write
time.read
random.read
compute.hash
compute.regex
process.args
process.spawn
identity.assume
identity.grant
runtime.native
runtime.unsafe
build.execute
build.network
container.privileged
container.host_access
k8s.rbac.grant
storage.persistent_read
storage.persistent_write
telemetry.emit
unknown
```

Provider-specific actions map into categories. For example:

```text
aws s3:GetObject       -> object_storage.read
aws s3:PutObject       -> object_storage.write
aws s3:DeleteObject    -> object_storage.delete
aws sts:AssumeRole     -> identity.assume
k8s RBAC get secrets   -> secret.read
k8s hostPath mount     -> container.host_access / filesystem.read/write depending mode
```

### 6.3 Versioned ontology

The capability ontology is versioned:

```text
reir.capability_ontology.v0.2
```

A REIR bundle must state which ontology version it uses. Adapters may emit
provider-specific capabilities even when a category mapping is unavailable, but
those facts are less comparable until mapped.

### 6.4 Required, granted, and observed capability facts

The same capability object can appear in different fact roles:

```text
required  code/package/build needs it
granted   identity/environment/policy grants it
observed  runtime observed it
```

Example required fact:

```json
{
  "kind": "capability",
  "role": "required",
  "capability": {
    "category": "object_storage.write",
    "provider": "aws",
    "action": "s3:PutObject",
    "resource": "arn:aws:s3:::reports-prod/exports/*"
  }
}
```

Example granted fact:

```json
{
  "kind": "capability",
  "role": "granted",
  "capability": {
    "category": "object_storage.write",
    "provider": "aws",
    "action": "s3:PutObject",
    "resource": "arn:aws:s3:::reports-prod/*"
  }
}
```

### 6.5 Capability coverage

A granted capability covers a required capability when the grant is at least as
permissive as the requirement for action, provider, resource, and constraints.

REIR does not define full provider IAM semantics. Instead:

```text
1. Provider adapters should emit canonical coverage facts when they can decide.
2. Reconciliation engines may implement provider-specific coverage algorithms.
3. If coverage cannot be decided, the result is partial or unknown, not safe.
```

Example:

```text
granted:  s3:PutObject on arn:aws:s3:::reports-prod/*
required: s3:PutObject on arn:aws:s3:::reports-prod/exports/*
coverage: covered
```

Example unknown:

```text
granted:  action from IAM policy with unresolved condition
required: s3:PutObject on symbolic bucket variable
coverage: unknown or partial depending available constraints
```

---

## 7. Edge and Flow Model

An `Edge` records a relation between two subjects or between a subject and a
capability/fact.

Minimal edge:

```json
{
  "schema": "reir.edge.v0.2",
  "id": "edge.checkout.upload.requires.s3_put_object",
  "kind": "requires_capability",
  "from": { "kind": "code.function", "id": "checkout::ReportUploader.upload" },
  "to": { "kind": "capability", "id": "cap.aws.s3.PutObject.reports-prod.exports" },
  "confidence": { "level": "computed", "source": "sdk_mapping" },
  "acquisition_mode": "source_scan",
  "precision": "resource_scoped",
  "evidence": [
    { "kind": "source_span", "file": "src/report_upload.rs", "line": 88 }
  ]
}
```

### 7.1 Core edge kinds

```text
calls
may_call
protocol_static_call
implements_protocol
normalizes_to_native_fn
requires_capability
grants_capability
observes_capability
denies_capability
uses_resource
opens_resource
retains
mutates
crosses_native
crosses_unsafe
depends_on
built_from
built_as
deployed_as
runs_as
assumes_role
routes_to
mounts_secret
mounts_volume
has_rbac_binding
has_policy_attachment
emits_event
unknown_edge
```

### 7.2 Flow graph

A flow graph is a generated, descriptive graph over REIR subjects and edges. It
is not executable and does not define program semantics.

Flow graph rules:

```text
1. Flow is generated from facts and edges; users do not author flow as behavior.
2. Flow edges must carry evidence.
3. Flow may be conservative: may-call and may-reach are valid.
4. Conditions may be labels, not executable expressions.
5. Flow is mainly reviewed through slices and diffs.
```

### 7.3 Cross-layer flow

The most important REIR flows cross layers:

```text
code.function
  -> requires_capability
  -> capability
  -> built_as container.image
  -> deployed_as k8s.deployment
  -> runs_as k8s.service_account
  -> assumes_role cloud.role
  -> grants_capability / missing grant
```

This is the flow that single-layer tools usually cannot see.

---

## 8. Reconciliation Model

Reconciliation compares facts with compatible capability keys and subject chains.

### 8.1 Required vs granted

```text
required - granted = missing capability
```

A missing capability is primarily an availability/deployment risk: the code or
build step may fail at runtime because the environment does not grant what it
needs.

Example:

```text
code requires s3:PutObject
role lacks s3:PutObject
result: missing_capability
```

### 8.2 Granted vs required

```text
granted - required = excess capability
```

An excess capability is primarily a security/least-privilege risk: the
environment grants more than the code appears to need.

Example:

```text
role grants s3:DeleteObject
no code/package/build fact requires s3:DeleteObject
result: excess_capability
```

### 8.3 Observed vs required or granted

```text
observed - required = unexpected behavior
observed - granted  = unauthorized or out-of-model behavior
required - observed = unused or unobserved required capability
```

Runtime observation is coverage-dependent. Absence of observed behavior is not a
proof of absence unless the observation source states sufficient coverage.

### 8.4 Reconciliation result schema

```json
{
  "schema": "reir.reconciliation.v0.2",
  "id": "recon.checkout.missing.s3_put_object.prod",
  "kind": "missing_capability",
  "status": "fail",
  "target": "prod checkout-api",
  "subject_chain": "chain.checkout.report_uploader.prod",
  "required_fact": "fact.checkout.report_uploader.requires.s3_put_object",
  "granted_facts": [],
  "capability": {
    "category": "object_storage.write",
    "provider": "aws",
    "action": "s3:PutObject",
    "resource": "arn:aws:s3:::reports-prod/exports/*"
  },
  "risk": {
    "class": "availability",
    "severity": "high",
    "reason": "deployment_target_does_not_grant_required_capability"
  },
  "evidence": [
    { "kind": "source_span", "file": "src/report_upload.rs", "line": 88 },
    { "kind": "rendered_manifest_pointer", "file": "rendered/prod/deployment.yaml", "json_pointer": "/spec/template/spec/serviceAccountName" },
    { "kind": "cloud_policy_pointer", "provider": "aws", "policy_arn": "arn:aws:iam::123456789012:policy/checkout-prod" }
  ]
}
```

### 8.5 Result kinds

```text
covered                  required capability is covered by grant
missing_capability       required capability lacks sufficient grant
excess_capability        grant has no corresponding requirement
unexpected_observation   runtime observed behavior not required/declared
unauthorized_observation runtime observed behavior not covered by grant
unused_capability        grant or requirement not observed in a stated window
partial_coverage         grant partially covers requirement
unknown_coverage         coverage cannot be determined
chain_incomplete         subject chain is incomplete, so reconciliation cannot complete
```

### 8.6 Security and availability are two directions of one comparison

REIR treats security and availability as two directions of capability comparison:

```text
missing grant  => availability/deployment risk
excess grant   => security/least-privilege risk
unexpected observed behavior => drift/security/modeling risk
```

This symmetry is a core design property.

---

## 9. Diff and Baseline Model

REIR review is diff-first.

### 9.1 Baseline

A baseline is an accepted set of REIR facts, edges, subject chains,
reconciliation results, profile rules, exceptions, and producer metadata.

A baseline may be stored as:

```text
review/reir-baseline.json
review/reir-baseline.lock
registry/attestation
CI artifact
```

### 9.2 Diff item kinds

```text
fact_added
fact_removed
fact_changed
edge_added
edge_removed
edge_changed
subject_chain_added
subject_chain_removed
subject_chain_changed
reconciliation_added
reconciliation_removed
reconciliation_changed
slice_added
slice_removed
slice_changed
diff_added
diff_removed
diff_changed
policy_result_added
policy_result_removed
policy_result_changed
profile_rule_changed
exception_added
exception_expired
exception_changed
schema_changed
producer_changed
ontology_changed
```

### 9.3 Semantic diff principle

The default reviewer question is:

```text
What changed semantically since the accepted baseline?
```

Full REIR graphs are machine data. Human review should start from semantic diff
and review slices.

### 9.4 Diff example

```text
REIR DIFF

+ required capability
  code: ReportUploader.upload
  capability: aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*
  evidence: src/report_upload.rs:88

+ reconciliation failure
  kind: missing_capability
  target: prod checkout-api
  chain: ReportUploader.upload -> checkout-api image -> Deployment/prod/checkout-api -> ServiceAccount/checkout-api -> IAM role checkout-prod
  reason: role does not grant s3:PutObject

review result:
  fail
```

---

## 10. Review Slices

A slice is a review-focused subset of REIR.

### 10.1 Slice kinds

```text
missing_capability_slice
excess_capability_slice
unexpected_observation_slice
network_slice
public_ingress_slice
object_storage_slice
database_slice
secret_slice
env_slice
time_slice
randomness_slice
compute_slice
telemetry_slice
process_slice
async_slice
diagnostic_slice
package_feature_slice
provider_implementation_slice
filesystem_slice
identity_slice
rbac_slice
storage_slice
build_time_slice
native_unsafe_slice
package_risk_slice
runtime_drift_slice
subject_chain_slice
diff_slice
unknown_slice
```

### 10.2 Slice rules

```text
1. Default human review should show slices, not the full graph.
2. A slice must include enough evidence to explain why it exists.
3. A slice may include hidden expandable details.
4. A slice should be diffable.
5. A slice may cross producers and repos.
```

Current REIR tooling derives slices from both reconciliation results and
standalone facts/edges. For example, RSScript package bundles produce
`package_risk_slice` from package/dependency risk facts and
`env_slice` from environment capability facts,
`time_slice` from wall-clock and monotonic-time capability facts,
`compute_slice` from hash and regex capability facts,
`telemetry_slice` from log and telemetry emission capability facts,
`process_slice` from process/thread-spawn capability facts, and
`async_slice` from await-site and async-boundary facts,
`diagnostic_slice` from compiler/package diagnostics,
`package_feature_slice` from selected package feature facts,
`provider_implementation_slice` from interface-provider implementation facts,
`native_unsafe_slice` from runtime native/unsafe capability facts, native/unsafe
boundary facts, selected native Cargo feature facts, declared native author
capabilities, and native/unsafe crossing edges even before cross-layer
reconciliation has run. RSScript-generated REIR bundles include these derived
slices in the bundle's top-level `slices` array at collection time. `reir slice`
can recompute or filter the same view from the bundle facts and edges. `reir
merge` also rebuilds derived slices after combining bundles so merged CI views do
not keep stale producer-local slices.

### 10.3 Example slice

```text
MISSING CAPABILITY SLICE

code:
  ReportUploader.upload requires aws.s3.PutObject on reports-prod/exports/*
  evidence: src/report_upload.rs:88

deployment chain:
  image: registry.example.com/checkout@sha256:abc...
  deployment: Deployment/prod/checkout-api
  service account: ServiceAccount/prod/checkout-api
  role: arn:aws:iam::123456789012:role/checkout-prod
  evidence: rendered/prod/deployment.yaml, terraform/prod/iam.tf

result:
  missing grant; deployment likely fails at runtime
```

---

## 11. Policy, Profile, and Exceptions

REIR can be evaluated against policy or profile, but REIR is not itself a policy
language.

### 11.1 Simple profile shape

Profiles should remain simple:

```text
allow
deny
review
budget
exception
```

Example:

```toml
[review.profile]
kind = "web-service"

[review.profile.allow]
network.public_ingress = true
object_storage.write = "review"
process.spawn = false
runtime.unsafe = false
unknown = false

[review.profile.budget]
max_missing_capabilities = 0
max_unknown_coverage = 0
max_excess_grants = 5
```

Complex condition languages are non-goals for REIR v0.2. External policy engines
may consume REIR facts if projects need richer policy.

### 11.2 PolicyResult

```json
{
  "schema": "reir.policy_result.v0.2",
  "id": "policy.prod.checkout.missing_capability.fail",
  "kind": "policy_result",
  "status": "fail",
  "policy": "prod-deployment-profile",
  "subject": { "kind": "application", "id": "checkout" },
  "reconciliation": "recon.checkout.missing.s3_put_object.prod",
  "reason": "max_missing_capabilities exceeded",
  "evidence": [
    { "kind": "unknown_reason", "reason": "missing_capability_count=1 budget=0" }
  ]
}
```

### 11.3 Exceptions and review debt

Exceptions allow adoption without pretending legacy risk is safe.

```toml
[review.exceptions."checkout-prod:s3_delete_excess"]
accepted_by = "platform-security"
reason = "legacy cleanup job still being removed"
expires = "2026-12-31"
```

Exception rules:

```text
1. Exceptions must have owner, reason, evidence, and expiry.
2. Expired exceptions fail review.
3. New risk not covered by an exception fails or requires review.
4. Exceptions are review facts and are included in diff.
```

---

## 12. Adapter Model

A REIR adapter is a producer that emits REIR from a source domain.

### 12.1 Adapter conformance levels

```text
Level 1: Best-effort adapter
  Emits inferred/scanned facts from raw artifacts.
  Useful for existing languages and systems.

Level 2: Contracted adapter
  Emits declared/computed facts from structured metadata or contracts.
  Useful for SDK mappings, package metadata, manifests, and plans.

Level 3: Native review-first producer
  Emits authoritative facts because the source model was designed to expose them.
  RSScript language facts are the reference example.
```

### 12.2 Adapter requirements

Every adapter must report:

```text
adapter name and version
input artifacts and hashes where available
schema version
ontology version
fact confidence and acquisition mode
coverage limitations
unknown reasons
```

### 12.3 Adapter coverage statement

Adapters should emit a coverage statement:

```json
{
  "schema": "reir.coverage.v0.2",
  "producer": "k8s-rendered-manifest",
  "covers": ["k8s desired state", "service account binding", "secret mounts"],
  "does_not_cover": ["mutating admission webhook result", "cloud load balancer final state"],
  "unknowns": ["service mesh sidecar injection not evaluated"]
}
```

This prevents reviewers from mistaking desired-state facts for full runtime truth.

---

## 13. RSScript Language Producer Profile

RSScript is a high-confidence REIR producer for application source semantics.

### 13.1 Produced facts

The RSScript frontend may emit authoritative REIR facts for:

```text
read / mut / take data effects
retention effects: retains(x)
managed closure capture retention
resource open / with behavior
fresh return contracts
local / manage transitions
native boundaries
unsafe boundaries
static protocol declarations and method contracts
protocol impl mappings
protocol_static_call edges
module declarations and use/import declarations
native module declarations and normalizes_to_native_fn edges
review-map classification: unknown / must_review / review_if_changed / low_semantic_risk
frontend diagnostics
source spans
```

### 13.2 Boundary

RSScript does not define infrastructure grants or runtime observations. RSScript
emits what code requires or exposes. K8s/IaC/IAM/runtime adapters emit what the
environment grants or observes.

### 13.3 Static protocol facts

Static protocol contracts demonstrate that REIR can represent constrained
abstraction without collapsing to unknown.

```text
protocol method contract known
  -> protocol_static_call fact/edge is known
  -> effects are bounded by protocol contract
  -> dynamic dispatch remains future/unsupported unless another producer owns it
```

### 13.4 Native module facts

Native module declaration groups demonstrate that REIR can represent grouped
native boundaries at package scale.

```text
native module File
  -> normalizes_to_native_fn File.open
  -> normalizes_to_native_fn File.open_write
  -> native boundary facts remain source/interface-backed
```

Module and `use` declarations are source-organization facts, not hidden behavior
facts. RSScript review-map JSON carries a top-level `modules` list with exact
source module paths, source locations, and fully qualified `use` paths; the REIR
RSScript adapter converts those entries into `module_declaration` and
`use_declaration` facts. Facts such as `module rss.package.review` and
`uses rss.review.ReviewMap` let review slices group large codebases by module,
but they must not imply Rust module semantics, import-time execution, implicit
method resolution, or additional capabilities.

---

## 14. RSScript Package Producer Profile

RSScript package tooling emits package and graph facts.

### 14.1 Produced facts

```text
.rssi public contract identities as `public_contract` facts
effective interface hashes
package feature selections
direct dependency identities as dependency_risk facts
direct dependency relationships as depends_on edges
await-site async boundaries as async_boundary facts
compiler/package diagnostics as diagnostic facts
selected package features as package_feature facts
selected native Cargo features as native_cargo_feature facts
provider implementation declarations as provider_implementation facts
package risk
native wrapper facts
binding manifest facts
build-time execution facts
proc macro / native link / FFI facts when known
unknown native facts with acquisition reason
dependency paths
lockfile evidence
capability metadata declared by package or registry
protocol/interface metadata after compiler normalization
```

For RSScript packages, `public_contract` facts cover the normalized public
contract entries emitted by package review metadata: public functions, structs,
resources, classes, sum types, type aliases, constants, and explicit protocol
implementations. This lets REIR diffs report data-model and typed-error
contract changes even when no native or unsafe boundary changed.
Protocol declarations should also produce `protocol_declaration` facts, and
their normalized method signatures should produce `protocol_method_contract`
facts. These facts are compiler-contract evidence about explicit
`Protocol.method(...)` capability contracts, not Rust trait method lookup. The
RSScript package-review evidence for a protocol declaration must carry the
effect-bearing method contracts, while `protocol_method_contract` facts may use
the normalized function export for each `Protocol.method` entry as their exact
method-signature evidence.
Explicit protocol implementation entries should also produce `protocol_impl`
facts and `implements_protocol` edges, so protocol conformance can be reviewed
as a constrained capability contract rather than hidden Rust trait behavior.
If the package review export is itself a contract diagnostic or has unknown
classification because the `.rssi` contract did not parse or type-check, the
corresponding `public_contract` fact must use `unknown` value and unknown
confidence. It must not assert a valid public contract merely because the broken
contract artifact was present.

The RSScript package producer should emit direct manifest dependencies even when
the dependency has not been resolved. A resolved local path dependency may carry
scanned confidence; unresolved registry or git dependencies must remain unknown
or lower-confidence facts rather than being treated as reviewed. The dependency
edge records package-shape evidence (`rsspkg.toml` / package review metadata);
transitive graph risk and effective interface hashes come from graph, lockfile,
or registry producers when those artifacts are available. The implemented
RSScript package-check producer (`rss pkg ci --json`) emits CI gate
`policy_result` facts for overall status, graph, lock, and native checks; stale
lock package-change facts and changed lock-field facts; provider implementation
declarations from `[implements]`; native unsafe/build-time facts; and
diagnostics with package-check evidence. This producer describes the package
manager gate result and does not replace lower-level package review or lockfile
producers. Overall status and graph policy facts use the package directory as
their evidence file. The lock policy fact uses `lockfile_entry` evidence at the
reported semantic lock path so CI can link directly to `rsspkg.lock`. The native
policy fact and native unsafe/build-time facts derived from `native_rust` use the
native wrapper path as their evidence file so CI can link directly to the native
review boundary instead of only the package root. Provider implementation facts
derived from `[implements]` use the package manifest (`rsspkg.toml`) as their
evidence file.
Package-check diagnostic facts preserve source-span evidence, and relative
diagnostic paths are resolved under the checked package directory so CI can open
the exact manifest, interface, source, native metadata, or policy file that
caused the diagnostic.
The implemented
RSScript lockfile producer (`rss pkg lock --reir`) emits package checksum,
effective interface hash, review metadata hash, and native wrapper hash as
`supply_chain` facts with `lockfile_entry` evidence and `lockfile` acquisition.
When the lockfile path is known, lockfile-entry evidence uses the concrete
semantic lock path (`<package-directory>/rsspkg.lock`); legacy or externally
collected lock JSON without a path may fall back to `rsspkg.lock`. If a lock
entry lacks an expected checksum, effective interface hash, review hash, or
native hash value, the corresponding `supply_chain` fact is `unknown` rather than
`true`; missing lock hashes must not be represented as verified lockfile
evidence.
Future lock-update REIR producers may emit update-risk, per-package risk, and
changed-field facts with `lockfile_entry` evidence so dependency update review
can be merged with package and graph evidence. The top-level
update-risk fact uses `/risk` evidence. Added or changed package/field evidence
points at the new lockfile; removed package or field evidence points at the old
lockfile because the reviewed entry no longer exists in the new artifact.
The implemented RSScript dependency-tree producer (`rss pkg tree --reir`) emits
transitive dependency-risk facts, effective-interface hash `supply_chain` facts,
and `depends_on` edges with `dependency_path` evidence for each resolved or
unresolved graph node. These facts and edges identify `rsscript_tree` as their
evidence source so merged bundles can distinguish graph observations from
package-review evidence. For resolved `path+` graph nodes, `dependency_path`
evidence uses the resolved package directory as its evidence file; unresolved
registry, git, or missing path nodes leave `evidence.file` empty rather than
inventing a local artifact path.
The implemented RSScript metadata producer (`rss pkg metadata --reir`, including
`--verify`) emits metadata status, generated-artifact `supply_chain` facts, and
stale/missing/unreadable mismatch `policy_result` facts with `package_metadata`
evidence. The top-level metadata status fact uses the package directory as its
evidence file and `/ok` as its JSON pointer. Metadata mismatch inputs include
the artifact kind, expected SHA-256, and actual SHA-256 when the stale artifact
was readable; REIR evidence preserves those digests in the mismatch value/reason
so CI can gate stale review artifacts without scraping human text. Mismatch
evidence files remain the artifact paths themselves; hash details must not be
appended to `evidence.file`. This producer describes whether review artifacts are
current; it does not replace the review bundle stored at `review/reir/rsscript.json`.
Generated REIR artifacts under `review/reir/` are review evidence, not package
payload.

Await-site metadata from package review should be preserved as `async_boundary`
facts. The fact subject is the enclosing RSScript function, evidence points at
the `await` span, and evidence detail records the classified boundary
(`runtime_pending`, `native_pending`, `rss_call`, or `unknown`), awaited callee,
and values live across the suspension point. These facts are compiler-contract
evidence about RSScript source, not runtime scheduling observations. Structured
`task_group` awaits should resolve lexical async-let handles back to their
initializer callee before emission, so a fact for `await user` records the
underlying `fetch_user(...)` boundary instead of losing the call target.

Package review diagnostics should be preserved as `diagnostic` facts. Diagnostic
facts carry the compiler/package diagnostic code, severity, summary, and primary
source span when available. Error diagnostics may use an `unknown` fact value so
policy and registry views can distinguish packages whose public contract could
not be fully classified from packages with only warning-level diagnostics.

Package feature selections should be preserved as `package_feature` facts with
`package.feature` subjects. Provider package declarations from `[implements]`
should be preserved as `provider_implementation` facts that record the
interface package name, interface version requirement, selected interface
features, and `interface_effective_hash` when present. Missing provider
interface hashes should remain visible as unknown provider implementation facts
instead of being collapsed into package risk text.

Native Rust Cargo feature selections should be preserved separately as
`native_cargo_feature` facts. These facts are package implementation metadata
about the selected Rust build shape, not RSScript public API feature contracts
and not proof that a native behavior is present or absent.

Native boundary groups should also produce `native_module_declaration` facts and
`normalizes_to_native_fn` edges from the native module subject to each normalized
native function subject. Package review may infer this grouping from explicit
`native module` syntax or from grouped native function namespaces, but the facts
remain source/interface-backed review boundaries rather than Rust module claims.

### 14.2 Boundary

Package tooling does not infer RSScript effects from Rust implementation. It
consumes compiler-normalized `.rssi` contracts and native/package metadata.
Native semantic behavior beyond declared contracts remains review-only unless
adapter/audit evidence exists.

When RSScript package review includes native Rust `source_scan_best_effort`
metadata, the package producer should preserve it as scanned REIR evidence, not
as a static RSScript safety guarantee. Detected Rust `unsafe` produces an
`unsafe_boundary` fact and a `runtime.unsafe` capability with scanned
confidence; detected `extern "C"` / `extern "system"` FFI produces a scanned
`native_boundary` fact and `runtime.native` capability; detected filesystem,
network, worker-thread parallelism, or build-script behavior produces scanned
capability facts such as `filesystem.read`, `network.client`, `process.spawn`,
or `build.execute`. Detected native build scripts should also produce
`build_time_execution` facts over a `build.step` subject so package and registry
views can distinguish build-time behavior from runtime capability requirements.
Absence of a detected pattern is not proof of absence unless a stronger audit
producer states coverage.

Native Rust `author_declaration` metadata is a different evidence channel from
`source_scan_best_effort` metadata and must not be collapsed into it. Author
declarations are manual/declared evidence about intended native behavior; for
example, `worker_thread_parallelism = true` produces a declared
`process.spawn` capability fact with acquisition mode `manual_declaration`.
The separate source scan may also detect worker-thread parallelism and emit a
scanned `process.spawn` capability fact. Keeping both facts lets reviewers see
where an author declaration and a best-effort implementation scan agree or
diverge.

### 14.3 Package capability metadata

Packages may declare capability mapping for public APIs. The mapping is evidence
for code-required capability when a call to that API is resolved.

Example:

```toml
[capabilities]
"S3.put_object" = ["aws.s3.PutObject"]
"File.open" = ["filesystem.read", "filesystem.write"]
"File.read_all_string" = ["filesystem.read"]
"File.open_write" = ["filesystem.write"]
"File.write_buffer" = ["filesystem.write"]
"Json.parse_file" = ["filesystem.read"]
"Toml.parse_file" = ["filesystem.read"]
"Env.get" = ["env.read"]
"Env.get_or_default" = ["env.read"]
"Env.current_dir" = ["env.read"]
"Env.home_dir" = ["env.read"]
"Env.temp_dir" = ["env.read"]
"Env.set" = ["env.write"]
"Env.set_current_dir" = ["env.write"]
"Http.get" = ["network.client"]
"Http.post_json" = ["network.client"]
"Http.post_form" = ["network.client"]
"Args.get_or_default" = ["process.args"]
"Process.run_stdout" = ["process.spawn"]
"Clock.now" = ["time.read"]
"Clock.system_unix_ms" = ["time.read"]
"Instant.elapsed" = ["time.read"]
"Random.bytes" = ["random.read"]
"Uuid.new_v4" = ["random.read"]
"Csv.open_read" = ["filesystem.read"]
"Config.load" = ["filesystem.read"]
"RuleLoader.load_rules" = ["filesystem.read"]
"Image.load" = ["filesystem.read"]
"Image.save" = ["filesystem.write"]
"Hash.sha256_string" = ["compute.hash"]
"Hash.sha256_bytes" = ["compute.hash"]
"Hash.sha256_file" = ["compute.hash", "filesystem.read"]
"Regex.compile" = ["compute.regex"]
"Regex.is_match" = ["compute.regex"]
"Regex.find" = ["compute.regex"]
"Regex.captures" = ["compute.regex"]
"Regex.replace_all" = ["compute.regex"]
"Regex.split" = ["compute.regex"]
"Log.write" = ["telemetry.emit"]
"TempDir.new" = ["filesystem.write"]
"TempDir.new_in" = ["filesystem.write"]
"TempDir.path" = ["filesystem.read"]
"TempDir.keep" = ["filesystem.write"]
```

Package metadata may be authoritative only for its declared public contract, not
for arbitrary implementation behavior.

The RSScript package producer also maps known bundled stdlib façade exports to
REIR capability facts when they appear in package review metadata. Examples:
`Env.get` requires `env.read`, `Directory.write_string` and
`File.write_buffer` require `filesystem.write`, `File.read_all_string`,
`Json.parse_file`, and `Toml.parse_file` require `filesystem.read`,
`File.open` conservatively requires both `filesystem.read` and
`filesystem.write`, `Http.get`, `Http.post_json`, and `Http.post_form` require
`network.client`, `Clock.now`, `Clock.system_unix_ms`, and `Instant.elapsed`
require `time.read`, `Random.bytes` and `Uuid.new_v4` require `random.read`,
`Csv.open_read`, `Config.load`, and `RuleLoader.load_rules` require
`filesystem.read`, `Image.load` requires `filesystem.read`, and `Image.save`
requires `filesystem.write`. Database evidence uses the generic
`database.read` and `database.write` categories emitted by database-aware
adapters. `Hash.sha256_string` and `Hash.sha256_bytes` require `compute.hash`,
`Hash.sha256_file` requires both `compute.hash` and `filesystem.read`,
`Regex.compile`, `Regex.is_match`, `Regex.find`, `Regex.captures`,
`Regex.replace_all`, and `Regex.split` require `compute.regex`, `Log.write`
requires `telemetry.emit`, `TempDir.new`, `TempDir.new_in`, and `TempDir.keep`
require `filesystem.write`,
`TempDir.path` requires `filesystem.read`, and
`Args.get_or_default` requires `process.args`, while `Process.run_stdout`
requires `process.spawn`. These facts are still
compiler-contract evidence about the public façade surface; they are not a scan
of arbitrary Rust implementation behavior.
Deterministic encoding helpers such as `Base64.encode`, `Hex.decode`, and
`Url.encode_component` remain public/native contract evidence but do not emit a
separate external capability fact unless a future profile introduces a
compute-encoding capability category.
Purely in-memory façade helpers such as `Cache.*`, `RetainedImageStore.*`,
`Environment.*`, and `FunctionObject.*` similarly do not emit external
capability facts beyond their public contract, retention, and native-boundary
evidence.
Descriptor-cleanup helpers such as `OS.close` are trusted native/resource
internals over `Fd`; they may still appear as native/public contract evidence,
but they do not imply `filesystem.read`, `filesystem.write`, or
`runtime.native` external capability facts by themselves.

---

## 15. K8s / IaC / IAM Producer Profile

K8s and IaC are high-value REIR producers because they are structured and often
declarative.

### 15.1 Inputs

Preferred inputs:

```text
rendered Kubernetes manifests
Helm template output
Kustomize output
Jsonnet/CDK rendered output
Terraform plan JSON
Terraform state when needed
cloud IAM policy documents
cloud provider identity bindings
admission-controller dry-run output when available
```

Source templates should remain evidence, but facts should be derived from
rendered/plan/normalized desired state where possible.

### 15.2 Produced facts

K8s/IaC/IAM adapters may emit:

```text
granted capabilities from IAM/RBAC/policy
network.public_ingress from Ingress/LoadBalancer/NodePort
network.egress policy facts
service account and workload identity facts
secret read facts from secret mounts/envFrom/imagePullSecrets
persistent storage read/write facts from PVC/volume mounts
container privilege facts from securityContext
host access facts from hostNetwork/hostPID/hostPath
RBAC grants
image/provenance/supply-chain facts
subject chain edges: deployed_as, runs_as, assumes_role, grants_permission
unknown facts for webhooks/controllers/cloud behavior not evaluated
```

### 15.3 Desired state vs actual state

Rendered manifests and Terraform plans describe desired or planned state, not
complete runtime truth. Producers must label facts accordingly:

```text
desired_state
planned_state
applied_state
observed_state
```

Mutating admission webhooks, service mesh injection, controller behavior, cloud
load balancer final state, and live drift may require additional observed or
applied-state producers.

### 15.4 K8s evidence example

```json
{
  "kind": "capability",
  "role": "granted",
  "subject": { "kind": "k8s.service_account", "id": "prod/ServiceAccount/checkout-api" },
  "capability": {
    "category": "identity.assume",
    "provider": "aws",
    "action": "sts:AssumeRole",
    "resource": "arn:aws:iam::123456789012:role/checkout-prod"
  },
  "confidence": { "level": "computed", "source": "rendered_manifest" },
  "acquisition_mode": "rendered_manifest",
  "precision": "resource_scoped",
  "evidence": [
    {
      "kind": "rendered_manifest_pointer",
      "file": "rendered/prod/serviceaccount.yaml",
      "json_pointer": "/metadata/annotations/eks.amazonaws.com~1role-arn"
    }
  ]
}
```

---

## 16. Runtime Observation Producer Profile

Runtime observation producers emit observed facts.

### 16.1 Sources

```text
cloud audit logs such as CloudTrail
Kubernetes audit logs
service mesh traces
application telemetry
eBPF/network telemetry
sandbox logs
container runtime events
policy enforcement logs
```

### 16.2 Observed facts

Runtime facts may include:

```text
observed cloud API call
observed network destination
observed filesystem write
observed process spawn
observed secret read
observed denied operation
observed identity principal
observed workload/container/image
observed runtime drift
```

### 16.3 Coverage warning

Runtime observation is time-windowed and collection-dependent. Absence of
observed behavior must not be treated as proof of absence unless the producer
states sufficient coverage and policy accepts that coverage.

---

## 17. Existing Code / SDK Scan Producer Profile

Existing-language adapters can emit useful required facts, but usually at lower
confidence than RSScript.

### 17.1 Sources

```text
AST/type-aware source scan
framework route scan
SDK call mapping
dependency metadata
configuration files
AI-assisted inference
manual annotations
```

### 17.2 Produced facts

```text
SDK-required capabilities, e.g. AWS S3 PutObject
HTTP client/server usage
filesystem/env/process usage
database access
queue/topic access
unknown external effects
```

### 17.3 Confidence discipline

Existing-language facts must not pretend to be authoritative unless the adapter
is backed by a language/compiler contract or explicit package contract.

```text
raw Rust/TypeScript/Python scan -> scanned/inferred
SDK mapping -> computed/scanned
manual annotation -> declared
RSScript compiler fact -> authoritative
```

---

## 18. Future Zig / Systems Producer Profile

A future systems-layer producer may emit facts for Zig or Zig-like system code.

Example fact families:

```text
allocation / deallocation
raw pointer use
ownership transfer
syscall
may_block
panic / abort policy
FFI / ABI boundary
atomic ordering
lock order
device access
DMA or buffer ownership
unsafe-equivalent low-level operation
```

This profile is future-facing. It is included to show that REIR is not tied to
app code or infrastructure declarations.

---

## 19. MVP: Cross-Layer Capability Reconciliation

The first REIR product demonstration should be cross-layer capability reconciliation for
deployment safety.

### 19.1 MVP question

```text
Did this change make code require a capability that the target deployment environment does not grant?
```

Secondary question:

```text
Did this change grant the deployment environment a capability that code does not require?
```

### 19.2 Minimum inputs

```text
code capability facts
  from RSScript, package metadata, SDK mapping, or source scan

deployment identity facts
  from rendered K8s manifests or equivalent deployment config

grant facts
  from Terraform plan, cloud IAM policy, Kubernetes RBAC, or provider analyzer

subject chain
  code/package -> image -> workload -> service account -> role/policy
```

### 19.3 Minimum output

```text
missing capabilities
excess capabilities
unknown coverage
subject chain evidence
semantic diff from baseline
small human-readable review slice
machine-readable REIR JSON
```

### 19.4 MVP report example

```text
REIR CAPABILITY RECONCILIATION

status: fail

missing capability:
  aws.s3.PutObject on arn:aws:s3:::reports-prod/exports/*

required by:
  ReportUploader.upload
  evidence: src/report_upload.rs:88

deployment chain:
  image: registry.example.com/checkout@sha256:abc...
  workload: Deployment/prod/checkout-api
  service account: ServiceAccount/prod/checkout-api
  role: arn:aws:iam::123456789012:role/checkout-prod
  evidence:
    rendered/prod/deployment.yaml#/spec/template/spec/serviceAccountName
    terraform/prod/iam.tf aws_iam_role.checkout_prod

reason:
  target role does not grant s3:PutObject for the required resource

impact:
  deployment likely fails at runtime when ReportUploader.upload runs
```

This MVP sells availability first: do not let a deployment fail because code and
infrastructure disagree. The same mechanism later supports least-privilege
security and runtime drift detection.

---

## 20. CLI and Integration Surface (Design Target)

REIR v0.2 does not require a specific CLI. The current prototype CLI supports
`collect --producer rsscript`, `reconcile`, `diff`, `slice`, `merge`, and `show`
over existing REIR bundle JSON. The RSScript collector accepts `--review-map`
JSON from a review-map artifact, `--package-review` JSON from `rss pkg review
--json`, or both; package-review JSON may also carry an embedded
`review_map`. It also accepts package-manager JSON artifacts from
`--package-check`, `--package-lock`, `--lock-update`, `--package-tree`, and
`--package-metadata`, then merges
the resulting RSScript producer bundles into one deduped bundle. When
`--package-lock` input JSON does not already carry `lockfile_path`, the
collector uses the input artifact path as lockfile-entry evidence so collected
bundles remain navigable in CI. Non-RSScript `collect` producers in the examples
below remain design targets.

```sh
reir collect --producer rsscript --package-review review/rss-package.json --out review/reir/rsscript.json
reir collect --producer rsscript --review-map review/rss-map.json --package-name my_package --out review/reir/rsscript-map.json
reir collect --producer rsscript --package-review review/package-review.json --package-check review/package-check.json --package-lock rsspkg.lock.json --out review/reir/rsscript-ci.json
rss pkg ci --json . > review/reir/rsscript-check.json
rss pkg metadata .   # writes review/package-review.json and review/reir/rsscript.json
rss pkg metadata --verify .   # verifies committed package review and REIR artifacts
rss pkg metadata --verify --reir . > review/reir/rsscript-metadata-verify.json
# planned non-RSScript producers:
# reir collect --producer k8s --from rendered/prod --out review/reir/k8s.json
# reir collect --producer terraform-plan --from tfplan.json --out review/reir/terraform.json
reir merge review/reir/*.json --out review/reir/system.json
reir reconcile --target prod --out review/reir/system-reconciled.json review/reir/system.json
reir diff --fail-on-change --baseline review/reir-baseline.json --current review/reir/system.json
reir slice --bundle review/reir/system-reconciled.json --kind package_risk
```

RSScript tooling provides REIR through the package subcommands that explicitly
support `--reir`, while REIR remains the common format rather than an
RSScript-only output. `reir diff` is the artifact path for comparing an already
locked REIR baseline with a current package REIR bundle.
`reir reconcile` supports the older two-bundle form
`--required required.json --granted granted.json [--target name]` and the
merged-bundle form shown above. In merged-bundle mode it reads required and
granted facts by `role`, writes reconciliation results back into the bundle when
`--out` is provided, and recomputes derived review slices.
When `--target <name>` is supplied, the implemented CLI records that target name
on each emitted `reir.reconciliation.v0.2` item and includes it in human output.
This is provenance for CI/review display; it does not filter facts by target.
`reir diff` compares facts, edges, subject chains, reconciliations, slices,
embedded diff artifacts, policy results, profile rules, exceptions, producer
metadata, bundle schema, and ontology. It defaults to
reporting semantic differences without failing; `--fail-on-change` makes any
diff item a non-zero CI result.
`reir slice` reads a bundle and can filter by any implemented slice kind listed
in §10.1. The filter accepts either the short form (`package_risk`) or the full
schema kind (`package_risk_slice`). Target-aware slicing remains a design target
until target scoping is represented in bundle subjects and reconciliation
results.

---

## 21. Schema Versioning and Bundles

A REIR bundle is a collection of subjects, chains, facts, edges, slices, policy
results, profile rules, reconciliation results, diffs, exceptions, and producer
metadata.

`subjects` is the bundle-level subject index. Producers should include each
subject referenced by facts, edge endpoints, and subject-chain nodes exactly once
by stable `id`. This index is redundant with fact and edge payloads, but it makes
merged bundles, graph views, and review slices usable without scanning every
fact first.

`reir merge` accepts only bundles with matching `schema` and `ontology`, dedupes
records by stable ids, rebuilds the `subjects` index from facts, edges, and
subject-chain nodes, keeps profile rules deduped by profile `kind`, and
recomputes derived review slices from the merged facts, edges, and
reconciliations.

Bundle skeleton:

```json
{
  "schema": "reir.bundle.v0.2",
  "ontology": "reir.capability_ontology.v0.2",
  "producers": [],
  "subjects": [],
  "subject_chains": [],
  "facts": [],
  "edges": [],
  "reconciliations": [],
  "slices": [],
  "policy_results": [],
  "profiles": [],
  "diffs": [],
  "exceptions": []
}
```

Schema evolution rules:

```text
1. New optional fields may be added in minor versions.
2. Existing required fields must not change meaning without a major schema bump.
3. Unknown extension fields must be preserved by tools that round-trip bundles.
4. Producers must state schema and ontology versions.
5. Diff tools must report schema and ontology changes as review events.
```

---

## 22. Security and Trust

REIR can become a sensitive artifact because it summarizes capabilities,
identities, secrets access, and deployment paths.

### 22.1 Integrity

REIR bundles should record input hashes where possible:

```text
source commit
rendered manifest hash
Terraform plan hash
policy document hash
container image digest
package lockfile hash
producer binary version/hash
```

High-assurance deployments may sign REIR bundles or attach them to provenance
attestations.

### 22.2 Confidentiality

REIR may reveal:

```text
internal service names
cloud account IDs
IAM role names
bucket names
secret names
network paths
runtime principals
```

Tools should support redaction for external sharing while preserving internal
traceability.

### 22.3 Tampering

A manually edited REIR fact is not authoritative unless accompanied by a manual
attestation and treated as declared/manual evidence. Producers should prefer
regeneration from source artifacts over trusting committed REIR facts.

---

## 23. Implementation Plan

### Phase 0: REIR core schema

```text
Fact
Evidence
Confidence / acquisition / precision
Capability ontology v0.2
SubjectChain
ReconciliationResult
Bundle
```

### Phase 1: First reconciliation demo

```text
code required capability from SDK mapping or RSScript fact
K8s workload/service account identity
Terraform/IAM granted capability
required vs granted reconciliation
missing capability report
```

### Phase 2: Diff and baseline

```text
REIR baseline
semantic diff
missing/excess capability diff
profile diff
exception handling
```

### Phase 3: Slices and CI UX

```text
missing capability slice
excess capability slice
subject chain slice
small human-readable report
machine-readable JSON for CI and AI agents
```

### Phase 4: Runtime observations

```text
CloudTrail/K8s audit observed facts
observed vs required/granted reconciliation
runtime drift slices
```

### Phase 5: High-confidence producers

```text
RSScript language/package producer
Zig/system producer
richer package capability metadata
```

---

## 24. Open Questions

```text
1. How canonical should provider-specific capability mappings be in v0.2?
2. Should provider IAM coverage be delegated to cloud-native analyzers or implemented in REIR tools?
3. What is the minimum stable subject chain for the first MVP?
4. How should symbolic resources be compared to concrete grants?
5. How should multi-environment overlays be represented without exploding fact volume?
6. How should runtime observation coverage be described and trusted?
7. What information should be redacted by default in external review reports?
8. When should REIR integrate with SBOM/provenance standards rather than reference them?
9. How should exceptions be governed across repos and teams?
10. What is the right boundary between REIR policy evaluation and external policy engines?
```

Not open for v0.2:

```text
- REIR is not executable.
- REIR does not define RSScript language or package semantics.
- Unknown is not safe.
- Required, granted, and observed facts are distinct roles.
- Flow is descriptive and evidence-backed, not user-authored program logic.
- Reconciliation must report the subject chain used for cross-layer comparison.
```

---

## 25. Summary

REIR is a common evidence layer for system behavior.

Its first-order value is not that many tools can dump facts into one JSON shape.
Its value is that facts from different layers can be connected into the same
subject chain, expressed in the same capability vocabulary, compared across
required/granted/observed roles, and diffed before or after deployment.

The central review shift is:

```text
from reviewing raw artifacts
  to reviewing evidence-backed semantic facts

from single-layer scanning
  to cross-layer reconciliation

from full re-review
  to semantic diff
```

The first brick is deployment-safety capability reconciliation:

```text
This change made code require X.
Does the target environment grant X?
If not, this deployment will likely fail at runtime.
```

RSScript remains a high-confidence producer of code semantics. K8s/IaC/IAM are
high-value structured producers for deployment and grant semantics. Runtime
telemetry supplies observed facts. REIR is the review evidence format that lets
all of them be reviewed together.
