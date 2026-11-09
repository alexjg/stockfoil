//! Verify shared executable code and the explicitly conditional graph models.
#[path = "../stockfoil/src/verified/clock.rs"]
pub mod clock;
#[path = "../stockfoil/src/verified/events.rs"]
pub mod events;
pub mod coordinates;
pub mod graph;
pub mod walk;
pub mod authorization;
pub mod authorization_tests;
pub mod visibility;
pub mod examples;
pub mod intervals;
pub mod materialization;
#[path = "../stockfoil/src/verified/tree.rs"]
pub mod tree;
pub mod tree_examples;
pub mod tree_refinement;
#[path = "../stockfoil/src/verified/difference.rs"]
pub mod difference;
pub mod difference_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/append_plan.rs"]
pub mod append_plan;

// The original suite also runs without these modules, with --no-cheating.
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/stdlib.rs"]
pub mod stdlib;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/candidates.rs"]
pub mod candidates;
#[cfg(stockfoil_stdlib)]
pub mod candidate_refinement;
#[cfg(stockfoil_stdlib)]
pub mod candidate_examples;
#[path = "../stockfoil/src/verified/chain_tables.rs"]
pub mod chain_tables;
#[cfg(stockfoil_stdlib)]
pub mod chain_table_refinement;

#[path = "../stockfoil/src/verified/storage.rs"]
pub mod storage;
pub mod storage_refinement;

#[path = "../stockfoil/src/verified/author.rs"]
pub mod author;
#[path = "../stockfoil/src/verified/chain_selection.rs"]
pub mod chain_selection;
pub mod chain_selection_refinement;

#[path = "../stockfoil/src/verified/commit_hash.rs"]
pub mod commit_hash;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/indexes.rs"]
pub mod indexes;
#[cfg(stockfoil_stdlib)]
pub mod index_refinement;

#[path = "../stockfoil/src/verified/author_key.rs"]
pub mod author_key;

#[path = "../stockfoil/src/verified/ancestry_walk.rs"]
pub mod ancestry_walk;
pub mod ancestry_refinement;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/ancestry_cache.rs"]
pub mod ancestry_cache;
#[cfg(stockfoil_stdlib)]
pub mod cache_examples;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/topology.rs"]
pub mod topology;
#[cfg(stockfoil_stdlib)]
pub mod topology_examples;
pub mod head_refinement;
pub mod capture_refinement;
pub mod lineage;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/captured.rs"]
pub mod captured;
#[cfg(stockfoil_stdlib)]
pub mod capture_examples;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/heads.rs"]
pub mod heads;
#[cfg(stockfoil_stdlib)]
pub mod head_examples;

#[path = "../stockfoil/src/verified/write_permission.rs"]
pub mod write_permission;
#[cfg(stockfoil_stdlib)]
pub mod permission_refinement;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/permissions.rs"]
pub mod permissions;
#[cfg(stockfoil_stdlib)]
pub mod permission_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/write_permissions.rs"]
pub mod write_permissions;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/identity.rs"]
pub mod identity;

#[cfg(stockfoil_stdlib)]
pub mod identity_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/api_error.rs"]
pub mod api_error;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/history_data.rs"]
pub mod history_data;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/history.rs"]
pub mod history;
#[cfg(stockfoil_stdlib)]
pub mod public_api_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/chain_intervals.rs"]
pub mod chain_intervals;
#[cfg(stockfoil_stdlib)]
pub mod interval_cursor_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/schedule_queues.rs"]
pub mod schedule_queues;
#[cfg(stockfoil_stdlib)]
pub mod scheduler_refinement;

#[path = "../stockfoil/src/verified/node_key.rs"]
pub mod node_key;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/pending.rs"]
pub mod pending;

#[cfg(stockfoil_stdlib)]
pub mod pending_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/traversal_step.rs"]
pub mod traversal_step;

#[cfg(stockfoil_stdlib)]
pub mod traversal_step_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/traversal_init.rs"]
pub mod traversal_init;

#[cfg(stockfoil_stdlib)]
pub mod traversal_init_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/traversal.rs"]
pub mod traversal;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/canonical_order.rs"]
pub mod canonical_order;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/commits.rs"]
pub mod commits;

#[cfg(stockfoil_stdlib)]
pub mod public_difference_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/traversal_extension.rs"]
pub mod traversal_extension;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/owned_traversal.rs"]
pub mod owned_traversal;

#[cfg(stockfoil_stdlib)]
pub mod owned_traversal_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/change_plan.rs"]
pub mod change_plan;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/view.rs"]
pub mod view;

#[cfg(stockfoil_stdlib)]
pub mod change_plan_examples;

#[cfg(stockfoil_stdlib)]
pub mod emission_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/batch_protocol.rs"]
pub mod batch_protocol;

#[cfg(stockfoil_stdlib)]
pub mod protocol_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/checkpoint.rs"]
pub mod checkpoint;

#[cfg(stockfoil_stdlib)]
pub mod checkpoint_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_source.rs"]
pub mod materialization_source;

#[cfg(stockfoil_stdlib)]
pub mod source_examples;

#[cfg(stockfoil_stdlib)]
pub mod history_equivalence;

#[cfg(stockfoil_stdlib)]
pub mod replay_examples;

#[cfg(stockfoil_stdlib)]
pub mod replay_plan;

#[cfg(stockfoil_stdlib)]
pub mod replay_ack;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_events.rs"]
pub mod materialization_events;

#[cfg(stockfoil_stdlib)]
pub mod event_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_feedback.rs"]
pub mod materialization_feedback;

#[cfg(stockfoil_stdlib)]
pub mod feedback_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_corrections.rs"]
pub mod materialization_corrections;

#[cfg(stockfoil_stdlib)]
pub mod correction_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_driver.rs"]
pub mod materialization_driver;

#[cfg(stockfoil_stdlib)]
pub mod driver_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/clock_equality.rs"]
pub mod clock_equality;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_start.rs"]
pub mod materialization_start;

#[cfg(stockfoil_stdlib)]
pub mod start_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/ledger.rs"]
pub mod ledger;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_plan.rs"]
pub mod materialization_plan;

#[cfg(stockfoil_stdlib)]
pub mod plan_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_complete.rs"]
pub mod materialization_complete;

#[cfg(stockfoil_stdlib)]
pub mod complete_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/document.rs"]
pub mod document;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/payload_resolution.rs"]
pub mod payload_resolution;

#[cfg(stockfoil_stdlib)]
pub mod resolution_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/drive_error.rs"]
pub mod drive_error;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materializer.rs"]
pub mod materializer;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_step.rs"]
pub mod materialization_step;

#[cfg(stockfoil_stdlib)]
pub mod step_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/progress.rs"]
pub mod progress;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_update.rs"]
pub mod materialization_update;

#[cfg(stockfoil_stdlib)]
mod update_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/draft.rs"]
pub mod draft;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/local_queries.rs"]
pub mod local_queries;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_local.rs"]
pub mod materialization_local;
#[cfg(stockfoil_stdlib)]
mod local_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/publication.rs"]
pub mod publication;
#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/document_publication.rs"]
pub mod document_publication;

#[cfg(stockfoil_stdlib)]
mod publication_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_edit.rs"]
pub mod materialization_edit;

#[cfg(stockfoil_stdlib)]
mod edit_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/commit_metadata.rs"]
pub mod commit_metadata;

#[cfg(stockfoil_stdlib)]
mod metadata_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/blake3_boundary.rs"]
pub mod blake3_boundary;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/commit_encoding.rs"]
pub mod commit_encoding;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/commit_builder.rs"]
pub mod commit_builder;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/document_commit.rs"]
pub mod document_commit;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_change.rs"]
pub mod materialization_change;

#[cfg(stockfoil_stdlib)]
mod builder_examples;

#[cfg(stockfoil_stdlib)]
mod change_examples;

#[cfg(stockfoil_stdlib)]
mod lifecycle_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/materialization_invariants.rs"]
pub mod materialization_invariants;

#[cfg(stockfoil_stdlib)]
mod pending_checkpoint_examples;

#[cfg(stockfoil_stdlib)]
mod counter_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/reader.rs"]
pub mod reader;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/metadata_encoding.rs"]
pub mod metadata_encoding;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/permission_codec.rs"]
pub mod permission_codec;

#[cfg(stockfoil_stdlib)]
pub mod codec_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/permission_map_codec.rs"]
pub mod permission_map_codec;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/view_codec.rs"]
pub mod view_codec;

#[cfg(stockfoil_stdlib)]
pub mod cursor_codec_examples;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/rejection_codec.rs"]
pub mod rejection_codec;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/checkpoint_parts.rs"]
pub mod checkpoint_parts;

#[cfg(stockfoil_stdlib)]
#[path = "../stockfoil/src/verified/checkpoint_codec.rs"]
pub mod checkpoint_codec;

#[cfg(stockfoil_stdlib)]
pub mod checkpoint_codec_examples;
