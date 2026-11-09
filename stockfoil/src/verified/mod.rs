//! Executable helpers compiled by Cargo and the Verus verification harness.
pub(crate) mod ancestry_cache;
pub(crate) mod ancestry_walk;
pub(crate) mod append_plan;
pub(crate) mod author;
pub(crate) mod author_key;
pub(crate) mod candidates;
pub(crate) mod captured;
pub(crate) mod chain_selection;
pub(crate) mod chain_tables;
pub(crate) mod clock;
pub(crate) mod commit_hash;
pub(crate) mod difference;
pub(crate) mod events;
pub(crate) mod heads;
pub(crate) mod identity;
pub(crate) mod indexes;
pub(crate) mod permissions;
pub(crate) mod stdlib;
pub(crate) mod storage;
pub(crate) mod topology;
pub(crate) mod tree;
pub(crate) mod write_permission;
pub(crate) mod write_permissions;

pub(crate) mod api_error;
pub(crate) mod history;
pub(crate) mod history_data;

pub(crate) mod chain_intervals;

pub(crate) mod schedule_queues;

pub(crate) mod node_key;

pub(crate) mod pending;

pub(crate) mod traversal_step;

pub(crate) mod traversal_init;

pub(crate) mod traversal;

pub(crate) mod canonical_order;

pub(crate) mod commits;

pub(crate) mod traversal_extension;

pub(crate) mod owned_traversal;

pub(crate) mod change_plan;

pub(crate) mod view;

pub(crate) mod batch_protocol;

pub(crate) mod checkpoint;

pub(crate) mod materialization_source;

pub(crate) mod materialization_events;

pub(crate) mod materialization_feedback;

pub(crate) mod materialization_corrections;

pub(crate) mod materialization_driver;
pub(crate) mod materialization_invariants;

pub(crate) mod clock_equality;

pub(crate) mod materialization_start;

pub(crate) mod ledger;

pub(crate) mod materialization_plan;

pub(crate) mod materialization_complete;

pub(crate) mod document;
pub(crate) mod payload_resolution;

pub(crate) mod drive_error;
pub(crate) mod materialization_step;
pub(crate) mod materializer;

pub(crate) mod materialization_update;
pub(crate) mod progress;

pub(crate) mod draft;
pub(crate) mod local_queries;
pub(crate) mod materialization_local;

pub(crate) mod blake3_boundary;
pub(crate) mod commit_builder;
pub(crate) mod commit_encoding;
pub(crate) mod commit_metadata;
pub(crate) mod document_commit;
pub(crate) mod document_publication;
pub(crate) mod materialization_change;
pub(crate) mod materialization_edit;
pub(crate) mod publication;

pub(crate) mod checkpoint_codec;
pub(crate) mod checkpoint_parts;
pub(crate) mod metadata_encoding;
pub(crate) mod permission_codec;
pub(crate) mod permission_map_codec;
pub(crate) mod reader;
pub(crate) mod rejection_codec;
pub(crate) mod view_codec;
