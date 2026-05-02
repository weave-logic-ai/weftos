---
title: Palantir Foundry Ontology — architecture research
source_urls:
  - https://www.palantir.com/docs/foundry/ontology/overview/
  - https://www.palantir.com/docs/foundry/ontologies/
  - https://www.palantir.com/docs/foundry/object-link-types/
  - https://www.palantir.com/docs/foundry/object-link-types/object-types-overview
  - https://www.palantir.com/docs/foundry/object-link-types/link-types-overview
  - https://www.palantir.com/docs/foundry/object-link-types/properties-overview
  - https://www.palantir.com/docs/foundry/object-backend/
  - https://www.palantir.com/docs/foundry/object-edits/
  - https://www.palantir.com/docs/foundry/action-types/overview/
  - https://www.palantir.com/docs/foundry/action-types/
  - https://www.palantir.com/docs/foundry/functions/
  - https://www.palantir.com/docs/foundry/functions/overview
  - https://www.palantir.com/docs/foundry/ontology-sdk/overview/
  - https://www.palantir.com/docs/foundry/vertex/
  - https://www.palantir.com/docs/foundry/machinery/
  - https://www.palantir.com/docs/foundry/object-explorer/overview/
  - https://www.palantir.com/docs/foundry/object-views/overview/
  - https://www.palantir.com/docs/foundry/object-permissioning/
  - https://www.palantir.com/docs/foundry/object-permissioning/ontology-permissions
  - https://www.palantir.com/docs/foundry/object-permissioning/managing-object-security
  - https://www.palantir.com/docs/foundry/ontology-manager/
  - https://www.palantir.com/docs/foundry/ontology-manager/overview/
redirect_notes:
  - /ontology/* content was reorganized under /object-link-types/, /object-backend/, /object-edits/, /ontologies/, /ontology-manager/, /action-types/, /functions/
  - Several deep pages returned 404 at the time of capture, notably /ontology-manager/manage-changes, /ontology-manager/ontology-cleanup, /object-edits/how-edits-are-applied, /object-edits/writebacks, /object-edits/edits-overview, /object-backend/user-edits, /object-backend/materializations, /object-backend/edit-history, /action-types/functions-backing-actions, /object-link-types/interfaces (trailing), /object-link-types/shared-properties (trailing), /ontology/limitations. Palantir's docs are navigable from sidebar trees; some of these pages may exist under slightly different slugs only reachable from an authenticated tenant.
created: 2026-04-24
scope: input for WeftOS ontology design; not an adoption plan
---

# Palantir Foundry Ontology — Architecture Research

## 1. Layered architecture

Foundry's Ontology is a stack sitting above raw data assets (datasets, virtual tables, models). Palantir describes it as a thing that "sits on top of the digital assets integrated into the Palantir platform (datasets, virtual tables, and models) and connects them to their real-world counterparts." The platform explicitly names two load-bearing halves — a **semantic layer** (object types, link types, properties, interfaces, shared properties) and a **kinetic layer** (action types, functions) — plus an orthogonal **governance layer** (Ontology Manager, permissions, writeback policies) and a **backend** (storage, indexing, query, edit application).

```
+------------------------------------------------------------+
|  User-facing applications                                  |
|  Object Explorer | Object Views | Vertex | Machinery       |
|  Workshop | Slate | Quiver | AIP agents | external OSDK    |
+------------------------------------------------------------+
                  |  reads / writes via OSDK / APIs
                  v
+------------------------------------------------------------+
|  Kinetic layer  (operations on the semantic layer)         |
|  Action Types     — user-facing edit transactions          |
|  Functions        — TypeScript / Python server-side logic  |
|  Side effects     — notifications, webhooks                |
+------------------------------------------------------------+
                  |  edits flow into object storage
                  v
+------------------------------------------------------------+
|  Semantic layer  (schema of the world)                     |
|  Object Types | Link Types | Properties                    |
|  Interfaces | Shared Properties | Struct / Value types     |
+------------------------------------------------------------+
                  |  defined / version-controlled by
                  v
+------------------------------------------------------------+
|  Governance layer — Ontology Manager                       |
|  resource CRUD | save / review / restore | permissions     |
|  usage tracking | cleanup | permission migration           |
+------------------------------------------------------------+
                  |  indexed & served by
                  v
+------------------------------------------------------------+
|  Object Backend                                            |
|  Ontology Metadata Service | Object Databases V1/V2        |
|  Object Data Funnel (writes) | Object Set Service (reads)  |
|  Actions Service (edit application)                        |
+------------------------------------------------------------+
                  |  derives from / writes back to
                  v
+------------------------------------------------------------+
|  Data assets — datasets, virtual tables, streams, models   |
+------------------------------------------------------------+
```

Per-layer responsibilities:

- **Data assets** — source of truth for bulk/batch data; the semantic layer is mapped onto these, not a substitute for them.
- **Object Backend** — physical durability and fast query. Object Databases "store the indexed object data in the Ontology and are designed to provide fast data querying." The Object Data Funnel "orchestrates data writes from datasources and user edits." The Object Set Service enables "searching, filtering, aggregating, and loading of objects." The Actions Service "applies user edits."
- **Governance** — Ontology Manager is the authoring surface; the Ontology Metadata Service stores "the set of ontological entities that exist."
- **Semantic layer** — schema-only definitions ("object types, link types, and action types define the schema of the ontology... not actual data values").
- **Kinetic layer** — the only sanctioned mutation path; captures intent, runs validation, emits side effects.
- **Applications** — consume the Ontology via OSDK/APIs; cannot bypass the kinetic layer for writes.

## 2. The semantic layer

### 2.1 Objects & Object Types

"An object type is a schema definition of a real-world entity or event, comprised of individual objects." An instance is an **object**; a selection is an **object set**. Palantir explicitly analogizes: object type ≈ table schema, object instance ≈ row, object set ≈ filtered rows.

Object types are created in the Ontology Manager "by adding backing datasources to an object type." The backing datasource supplies primary keys, property values, and (when configured) security context. An object type carries title key and primary key designations, property definitions, link endpoints, and display/descriptive metadata. Schema evolution goes through Ontology Manager change-management flows.

### 2.2 Properties

"A property of an object type is a characteristic that informs a real-world entity or event." Properties have a base type. Commonly used: `String`, `Integer`, `Short` (valid as title and primary key); `Date`, `Timestamp` (title key only — primary-key use discouraged due to "potentially unexpected collisions"); `Boolean`, `Byte`, `Long` (`Long` has "representational issues in JavaScript"); `Float`, `Double`, `Decimal` (title key only); plus structural / specialized types: `Vector`, `Array`, `Struct`, `Media Reference`, `Time Series`, `Attachment`, `Geopoint`, `Geoshape`, `Marking`, `Cipher`. "Array properties cannot contain null elements." Metadata includes display names, descriptions, value formatting, conditional formatting.

### 2.3 Link Types

"A link type is the schema definition of a relationship between two object types. A link refers to a single instance of that relationship between two objects in the same Ontology."

- **Cardinality** — one-to-many and many-to-many are supported. For many-to-many, "datasources back the link types themselves" rather than being inferred from foreign keys on the object types.
- **Self-links** — "Links can also exist between two objects of the same type."
- **Cross-ontology links** — "Links between object types across different Ontologies is not supported"; shared ontologies are the workaround.
- **Backing** — "Links are created and displayed in user applications by adding backing datasources to the object types referred to in the link type in the Ontology Manager."

### 2.4 Interfaces (polymorphism)

Interfaces "provide object type polymorphism, allowing for consistent modeling of and interaction with object types that share a common shape." An interface declares a shape (required properties, possibly required links). Multiple object types can implement the same interface, enabling generic actions/functions — one written against `IncidentLike` operates on `FireIncident`, `MedicalIncident`, etc. Interfaces are first-class ontology resources in the Ontology Manager.

### 2.5 Shared Properties

Shared properties "promote consistency across multiple object types" and enable "centralized management of property metadata." A shared property is a named, typed, described property definition (e.g. `email_address` with format and privacy marking) that multiple object types reference. Changing its metadata propagates to every consumer. Adjacent constructs value types and struct types standardize richer shapes.

## 3. The kinetic layer

### 3.1 Action Types

"An action is a single transaction that changes the properties of one or more objects, based on user-defined logic." An **action type** is "the definition of a set of changes or edits to objects, property values, and links that a user can take at once."

Components:

- **Parameters** — typed user inputs with defaults, dropdown filtering, overrides, security-aware object pickers.
- **Rules** — logic computing edits; declarative or function-backed.
- **Submission criteria** — validations that must hold before commit; block the write when they fail.
- **Side effects** — "notifications" and "webhooks" fired on submission; webhooks enable external-system integration.
- **Permissioning** — executed against the user's permissions; edits require "edit permissions on the action type and on all ontology resource types edited by the action." Link edits additionally require "edit permissions on both the link type and the linked object types."
- **Commit** — "Changes made to objects, property values, and links will be committed to the Ontology when the user takes the action" and appear in writeback datasets plus all applications. Key limitation: "Actions are not yet supported on object types with Foundry stream datasources."

Actions are the **only** user-facing mutation primitive — no app writes directly to the backend.

### 3.2 Functions

Functions "enable code authors to write logic that can be executed quickly in operational contexts, such as dashboards and applications designed to empower decision-making processes." Runtimes: **TypeScript** (v1 and v2) and **Python**. They run "server-side in isolated environments" (containers) with "first-class support for authoring logic based on the Ontology" — reading properties, traversing links, emitting edits.

Invocation paths:

- **Function-backed Actions** — function returns a set of edits the Actions Service applies.
- **Synchronous** — Workshop modules, dashboards, Slate, derived columns.
- **Asynchronous** — Pipeline Builder (Python as sidecar containers).
- **APIs / OSDK** — Query functions via API gateway.
- **Analytics** — Quiver aggregations and metrics.

Functions support versioning, published versions, monitoring/telemetry, role-based execution permissions, and unit-testing harnesses with stub objects.

### 3.3 Ontology Edits (transactions)

Every mutation is an Action, and every Action is "a single transaction that changes the properties of one or more objects." The Actions Service "applies user edits to object databases." The Object Data Funnel orchestrates "data writes from datasources and user edits."

(Deeper transactional semantics — isolation, conflict handling, ordering across concurrent actions, retry — live in /object-edits sub-pages that 404'd in this capture. Confirmed: atomic per-action commit, visibility to all applications on commit, emission to writeback datasets.)

### 3.4 Writebacks

Actions "appear in writeback datasets" after commit. Writeback datasets are dataset materializations of edits, consumable by downstream pipelines like any other dataset. This is how Ontology edits propagate back into source-of-truth systems: standard data-integration pipelines read writeback datasets and push to SAP, Salesforce, etc. Back-propagation reuses Foundry's data-integration machinery in reverse.

## 4. The governance layer — Ontology Manager

The Ontology Manager is Palantir's authoring/governance application. It lets teams "build and maintain your organization's Ontology." It manages: object types, link types, action types, function types, properties, shared properties, value types, struct types, interfaces, and object type groups.

**Change control**: "Save changes to the Ontology" and "Review and restore changes" are named surfaces. Changes are stageable, reviewable, restorable — a versioned edit log over ontology definitions, separate from object-data edits.

**Usage tracking**: every object-type view has a **Usage** section showing adoption across downstream applications (Workshop modules, Object Views, functions). Essential for safe schema evolution — you see who breaks before you change.

**Permissions / migration**: ontology resources live as projects in Compass (Foundry's filesystem), with Viewer/Editor/Owner roles; project-based permission migrations are supported.

**Cleanup**: deleting a type with live dependents requires explicit migration.

**Data model**: ontological entities are stored by the **Ontology Metadata Service**, which holds "the set of ontological entities that exist." An ontology itself is "an artifact which stores ontological resources or entities" including "object types, link types, action types, interfaces, shared properties, and object type groups." Ontologies can be "private and assigned to a single organization or shared among multiple organizations"; shared ontologies enable "users of different organizations to share data and workflows safely." Critically: "An ontology is mapped 1:1 with a space. When a new space is created, a corresponding ontology with the same name is simultaneously created." A private space → private ontology; shared space → shared ontology.

**Versioning / audit**: function versioning is explicit ("By default, the latest version of the function is displayed"). Broader ontology-definition versioning rides Save/Review/Restore. Per-object edit history is referenced in Object Backend docs but detailed pages 404'd here.

## 5. User-facing applications

### 5.1 Vertex

Vertex "allows you to visualize and quantify cause and effect across the digital twin of your real-world organization." A graph-shaped analytical UI over the Ontology: live network exploration across silos, alert/risk propagation, simulation ("combining analytics and operations to find the most impactful changes"), optimization. Builds on the entire semantic layer. **Without Vertex, the semantic graph is still queryable via OSDK/Object Explorer, but cross-functional visual exploration and scenario simulation disappear.**

### 5.2 Machinery

Machinery "enables you to understand and manage all aspects of a process, identify unwanted behavior, and make improvements toward a desired outcome." Models temporal, state-machine-shaped workflows — entities that "undergo changes of state over time." Capabilities: process mining from event logs, performance-metric definition, operational apps, AIP-assisted multi-step automation with human-in-the-loop.

Builds on: object types (entities), action types (state transitions), functions (logic), edit history/event log. **Without Machinery, process workflows can still be hand-built on actions and object views, but process-mining, bottleneck analytics, and orchestrated multi-step LLM flows go missing.**

### 5.3 Object Explorer

Object Explorer is "a search and analysis tool for answering questions about anything in the Ontology." Runs queries from "simple keyword searches to comprehensive property filters," renders tables, drills into Object Views, compares object sets, runs bulk actions, exports, persists saved explorations. Builds on the Ontology, Object Views, objects, properties, and filters. **Without Object Explorer, non-technical users lose point-and-click discovery; developers retain everything via OSDK/APIs.**

### 5.4 Object Views

Object Views are "reusable representations of object data" consolidating "key information about the object, including property data, object links, and related applications." Two types: Standard (autogenerated from schema) and Configured (built in Workshop). Two form factors: Full and Panel. They integrate with Actions — so the default mutation UX for any object type is a standard Object View. **Without Object Views, there is no default object-rendering UI; every tenant ships its own or drops to APIs.**

## 6. Writebacks & external systems

Ontology edits become writeback datasets automatically. These are ordinary Foundry datasets: pipeable back into source systems using the same connectors used for ingest, versioned like any other dataset, and consumable by downstream pipelines as input.

Transaction model: each Action commits atomically, producing one transaction on the relevant writeback dataset(s). Back-propagation to external systems is **not** synchronous with the Action commit — it is pipeline-mediated. Guarantees to source systems are therefore eventually consistent, with the writeback dataset as the auditable intermediary.

Webhooks (Action side effects) give a synchronous external notification at commit time, orthogonal to the writeback-dataset path.

Streaming gap: "Actions are not yet supported on object types with Foundry stream datasources." Streaming-backed object types are read-only via the kinetic layer.

## 7. Policy & permissions model

Foundry's permissions system is **project-based RBAC** layered with **marking-based ABAC**, with schema/data separation.

- **Policy location**: "Permissions to view, edit, and manage ontology resources are managed through Compass, the Palantir platform's filesystem." Ontology resources are Compass files governed by Viewer/Editor/Owner roles at folder or project level.
- **Two layers** — schema-level (type definitions) and data-level (instances).
- **Data security** uses two approaches:
  1. **Object and property security policies** (recommended) — near-instantaneous effect, cell-level via row + column combinations.
  2. **Data source policies** — Restricted Views (row filtering) and Multi-Data Source object types (column mapping across differently-permissioned sources); require pipeline rebuilds to change.
- **Granularity**: object, property, link, and cell-level. Link edits require permissions on the link type and both endpoints' object types. Action execution requires edit permissions on the action type plus every resource type it edits.
- **Inheritance**: "Data permissions for object types are implicitly controlled by the permissions applied to the input data sources of the object type"; Viewer on a backing dataset typically yields read on the object type unless policy narrows.
- **Markings** — Foundry's ABAC primitive; present on ontology resources and propagated through lineage. Cipher property type exists for encrypted-at-rest values.

## 8. Primitive operations that are truly primitive

What is load-bearing vs convenience sugar:

- **Primitive (irreducible):**
  - **Object Type** — schema definition keyed by primary key, with typed properties and backing datasources.
  - **Link Type** — binary relation schema; many-to-many variant needs its own backing datasource.
  - **Property** — named, typed, metadata-bearing cell.
  - **Action Type** — the only sanctioned mutation transaction; the kinetic primitive.
  - **Function** — the only sanctioned place for nontrivial business logic evaluated server-side against ontology state.
  - **Ontology Metadata Service** — the registry; without it, nothing else is nameable.
  - **Object Database + Object Set Service + Actions Service + Object Data Funnel** — the minimum backend to serve, query, and mutate object state.
  - **Marking / Permission** — the policy primitive; without it, there is no governance.

- **Derivable / convenience sugar:**
  - **Interfaces** — pattern sugar. Could be emulated by conventions ("every type with these properties is incident-shaped") but without type-checked polymorphism, generic actions/functions become brittle. Fundamental for safe reuse, not for core CRUD.
  - **Shared Properties** — governance sugar over copy-pasted property definitions. Achievable by linter/code-review discipline but at the cost of metadata drift at scale. Fundamental for multi-team ontologies, not for small ones.
  - **Object Type Groups** — pure organizational sugar.
  - **Value Types / Struct Types** — reuse sugar over base types; not strictly primitive.
  - **Writeback datasets** — an implementation of the Actions Service's output; edits could be streamed differently.
  - **Object Views (standard variant)** — a rendering of schema metadata; zero new information.
  - **Object Explorer** — a UI on top of the Object Set Service.

The minimal primitive set is: Object Type, Link Type, Property, Action Type, Function, Metadata Service, Backend (DB + OSS + Actions Service + Funnel), Marking/Permission. Everything else is built from those.

## 9. Known limitations / tradeoffs

- **Streaming-writes gap** — Actions are not supported on object types with Foundry stream datasources. Streaming-backed object types are read-only via the kinetic layer.
- **Cross-ontology linking** — not supported; workaround is shared ontologies, which forces organizational/political alignment to be a prerequisite for technical linking.
- **1:1 ontology ↔ space** — ontology boundary is coupled to Foundry's workspace construct. You cannot reorganize ontology boundaries independently of spaces.
- **Permission-system heaviness** — Compass RBAC + object/property policies + data-source policies + markings is four overlapping mechanisms; the "recommended" object/property security policy is near-instant but requires explicit opt-in, while data-source policies require pipeline rebuilds on change.
- **Long primary keys** — `Long`, `Date`, `Timestamp`, `Float`, `Double`, `Decimal` have discouragements or outright bans as primary keys; `Long` specifically has "representational issues in JavaScript."
- **Writeback is pipeline-mediated** — back-propagation to source systems is eventually consistent, not transactional with the Action commit. Synchronous integration needs webhooks, which are fire-and-forget.
- **Schema evolution cost** — deleting or renaming ontology resources requires explicit cleanup and migration; the Usage panel is the guard but does not automate the fix.
- **Coupling to Foundry backend** — every primitive (Object DB, OSS, Actions Service, Ontology Metadata Service, Object Data Funnel) is a Foundry service. The semantic layer is not portable; it is a description of a Foundry deployment.
- **Action Service as the only write path** — simplifies governance but makes high-throughput programmatic ingest awkward; bulk ingest goes through datasets, not actions.
- **Two function runtimes (TS + Python) only** — no Rust, Go, or WASM runtime for ontology-adjacent logic; foreign code must sit behind APIs/webhooks.

## 10. Cross-references for future WeftOS design

For each Foundry concept, the nearest WeftOS analog. **Mapping only; no design proposals.**

| Foundry concept | Closest WeftOS analog |
|---|---|
| Data assets (datasets, virtual tables, streams) | WeftOS substrate: path-keyed KV + pub/sub streams |
| Object Database | An index service built on top of substrate paths |
| Object Set Service | A query service over substrate indexes |
| Actions Service | A mutation service that sequences substrate writes |
| Object Data Funnel | Adapter ingest path into substrate |
| Ontology Metadata Service | A substrate-backed registry under a reserved path prefix |
| Object Type | Schema artifact registered in the registry; instances live under a path namespace |
| Property | Typed subpath under an object's path |
| Link Type | Relation schema; links either stored as pointer properties or in a dedicated link namespace |
| Interface | Generic shape declared in the registry; typed routing over object-type namespaces |
| Shared Property | Named property definition referenced by multiple object-type schemas in the registry |
| Action Type | Server-side handler registered against substrate paths with declared edits and side effects |
| Function | Executable unit invoked by actions and applications; candidate for WASM runtime |
| Writeback dataset | Substrate event-log stream of edits, consumable by adapters |
| Webhook side effect | Pub/sub event emitted on action commit |
| Markings | Tag/label scheme on substrate paths |
| Object/property security policy | Permission rules evaluated at the substrate boundary |
| Compass project-based RBAC | Project/namespace ACLs on substrate paths |
| Object View (standard) | Auto-rendered view derived from registry schema in the Explorer |
| Object View (configured) | Custom view shipped by a service/adapter |
| Object Explorer | WeftOS Explorer, querying over registered object types |
| Vertex | Not present in substrate; a graph-analytics service above the ontology |
| Machinery | Not present in substrate; a workflow/state-machine service above actions |
| Ontology Manager | A governance service (authoring + save/review/restore) over the registry |
| Ontologies as artifacts, 1:1 with space | A top-level namespacing construct mapping to a WeftOS tenant / snapshot boundary |
| Snapshot (Foundry has no direct analog) | WeftOS snapshot — point-in-time capture of registry + substrate state |

---

## Appendix A — Glossary of Foundry terms

- **Action** — a single user-invoked transaction that changes objects, properties, and/or links.
- **Action Type** — the schema/definition of an Action, including parameters, rules, submission criteria, and side effects.
- **Actions Service** — backend service that applies Action-produced edits to the Object Database.
- **AIP** — Palantir's AI Platform; LLM/orchestration layer consumed by Machinery.
- **Backing datasource** — the dataset, virtual table, or stream whose rows become an object type's objects (or a link type's links for many-to-many).
- **Cipher** — property type for encrypted-at-rest values.
- **Compass** — Foundry's filesystem and project/permission registry; ontology resources live as Compass files.
- **Function** — server-side logic (TypeScript or Python) with first-class ontology access; invokable from Actions, apps, APIs, Pipeline Builder.
- **Function-backed Action** — Action whose edits are computed by a Function.
- **Interface** — polymorphism construct; a shape multiple object types may implement.
- **Link / Link Type** — edge instance / edge schema between two object types.
- **Machinery** — process-mining and workflow-orchestration application on top of the ontology.
- **Marking** — access-control label; ABAC primitive.
- **Multi-Data-Source Object Type (MDO)** — object type assembling columns from multiple datasources with per-source permissions.
- **Object / Object Type** — instance / schema of a real-world entity or event.
- **Object Backend** — umbrella term for Object DB, OSS, Actions Service, Object Data Funnel, Ontology Metadata Service.
- **Object Database (V1/V2)** — storage/indexing service for object instances.
- **Object Data Funnel** — write-orchestration service into Object Databases.
- **Object Explorer** — search/analysis UI over the ontology.
- **Object Set / Object Set Service (OSS)** — filtered selection of objects / the service that implements set operations.
- **Object Type Group** — organizational grouping of object types in Ontology Manager.
- **Object View** — standard or configured UI for rendering a single object.
- **Ontology** — an artifact storing ontological resources (object types, link types, action types, interfaces, shared properties, object type groups); mapped 1:1 with a Space.
- **Ontology Manager** — authoring/governance application for ontology resources.
- **Ontology Metadata Service** — registry of all ontological entities that exist.
- **Ontology SDK (OSDK)** — generated, typed client library (TypeScript, Python, Java, OpenAPI) for reading and writing the ontology from external apps.
- **Pipeline Builder** — Foundry's batch/streaming transformation tool; Python functions may run as sidecar containers here.
- **Property** — characteristic of an object type, with a base type and metadata.
- **Quiver** — Foundry analytics app; consumes functions for custom aggregations.
- **Restricted View (RV)** — row-filtered dataset used for row-level access control.
- **Shared Property** — centrally defined property reused across multiple object types.
- **Side Effect** — notification or webhook emitted on Action submission.
- **Slate** — Foundry's low-code app-building tool.
- **Space** — Foundry's workspace construct; 1:1 with an Ontology.
- **Struct Type / Value Type** — composite and named base types used inside properties.
- **Submission Criterion** — validation that must hold for an Action to commit.
- **Vertex** — cause-and-effect / simulation visualization over the ontology.
- **Webhook** — outbound HTTP call triggered as an Action side effect.
- **Workshop** — Foundry's operational application builder; a primary consumer of Actions and Object Views.
- **Writeback Dataset** — dataset materialization of Ontology edits; consumable by downstream pipelines including those that back-propagate to source systems.
