//! Concrete Object Type implementations.
//!
//! Each submodule declares a single `ObjectType`. New types are added
//! by (1) creating a module here, (2) exporting it below, and (3)
//! inserting a dispatch branch at the
//! `[[OBJECT_TYPES_REGISTRATIONS_INSERT]]` marker in `super::infer`.
//!
//! See `.planning/ontology/ADOPTION.md` §8 Step 2 for the promotion
//! roadmap of substrate shapes to typed Object Types.

// [[OBJECT_TYPES_MODULES_INSERT]]
pub mod audio_stream;
pub mod chain_event;
pub mod health_report;
pub mod mesh;
pub mod node;
pub mod sensor;
