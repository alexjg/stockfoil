#!/usr/bin/env python3
"""Regression checks for the trust-boundary guardrail."""
import shutil
import tempfile
import unittest
from pathlib import Path
from check_trusted import ROOT, PUBLIC_API_WIRING, BLAKE3_DEPENDENCIES, audit


class TrustAudit(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for name in ['verification', 'stockfoil/src/verified']:
            shutil.copytree(ROOT / name, self.root / name, ignore=shutil.ignore_patterns("target", "__pycache__"))
        for name in PUBLIC_API_WIRING | BLAKE3_DEPENDENCIES:
            (self.root / name).parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(ROOT / name, self.root / name)

    def test_current_manifest(self):
        self.assertEqual(audit(self.root), 43)

    def test_public_replay_cannot_hide_unchecked_composition(self):
        for file, anchor in [
            ('replay_plan.rs', 'proof fn parent_members('),
            ('replay_plan.rs', 'pub(crate) proof fn plans_same'),
            ('replay_plan.rs', 'pub(crate) fn resumed_plan'),
            ('replay_plan.rs', 'pub(crate) fn public_partial_replay'),
            ('replay_ack.rs', 'pub(crate) fn acknowledge'),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'verification' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_reconstruction_equivalence_cannot_be_trusted(self):
        for file, anchor in [
            ('history_equivalence.rs', 'proof fn path('),
            ('history_equivalence.rs', 'pub(crate) proof fn authorized('),
            ('history_equivalence.rs', 'pub(crate) proof fn records_equivalent('),
            ('replay_examples.rs', 'pub(crate) fn reconstruct('),
            ('replay_examples.rs', 'pub(crate) fn persisted_partial_matches_uninterrupted('),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'verification' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_unchecked_public_checkpoint_codecs_are_rejected(self):
        for file, anchor in [
            ('checkpoint_codec.rs', '    pub fn to_bytes('),
            ('checkpoint_codec.rs', '    pub fn from_bytes('),
            ('checkpoint_codec.rs', '    fn decode('),
            ('checkpoint_parts.rs', 'pub(crate) fn decode_progress('),
            ('rejection_codec.rs', 'pub(crate) fn decode('),
            ('rejection_codec.rs', 'pub(crate) proof fn encoding_decodes('),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'stockfoil/src/verified' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_checkpoint_codec_cannot_be_shadowed_in_production(self):
        path = self.root / 'stockfoil/src/history/checkpoint.rs'
        path.write_text(path.read_text() + '\nimpl Checkpoint { pub fn to_bytes(&self) -> Vec<u8> { Vec::new() } }\n')
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_public_cursor_codec_is_rejected(self):
        for file, anchor in [
            ('view_codec.rs', '    pub fn to_bytes('),
            ('view_codec.rs', '    pub fn from_bytes('),
            ('view_codec.rs', '    fn decode_cursor('),
            ('permission_map_codec.rs', 'pub(crate) fn decode('),
            ('permission_map_codec.rs', 'pub(crate) proof fn encoding_decodes('),
            ('write_permissions.rs', '    pub(crate) fn ordered_keys('),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'stockfoil/src/verified' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_cursor_codec_cannot_be_shadowed_in_production(self):
        path = self.root / 'stockfoil/src/history/view.rs'
        path.write_text(path.read_text() + '\nimpl MaterializationCursor { pub fn from_bytes(_: &[u8]) -> Self { todo!() } }\n')
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_metadata_codecs_are_rejected(self):
        for file, anchor in [
            ('reader.rs', '    pub(crate) fn take('),
            ('reader.rs', '    pub(crate) fn count('),
            ('metadata_encoding.rs', 'pub(crate) fn encode_hashes('),
            ('permission_codec.rs', 'pub(crate) fn decode_permission('),
        ]:
            with self.subTest(file=file):
                path = self.root / 'stockfoil/src/verified' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_reader_cannot_bypass_checked_implementation(self):
        path = self.root / 'stockfoil/src/reader.rs'
        path.write_text(path.read_text().replace('crate::verified::reader::Reader', 'crate::unchecked_reader::Reader'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_counter_origin_and_invariants_are_rejected(self):
        for file, anchor in [
            ('materialization_feedback.rs', '    pub(crate) fn new(next: Checkpoint,'),
            ('materialization_invariants.rs', 'pub(crate) proof fn translated_inventory('),
            ('materialization_invariants.rs', 'pub(crate) proof fn round_preserves_counts('),
            ('materialization_invariants.rs', '    pub(crate) proof fn counter_consistency_definition()'),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'stockfoil/src/verified' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_unchecked_checkpoint_invariants_are_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_invariants.rs'
        original = path.read_text()
        for anchor in ['    pub(crate) proof fn checkpoint_consistency_definition()',
                       'pub(crate) proof fn rejection_count_union(',
                       'pub(crate) proof fn round_preserves_alignment(']:
            with self.subTest(anchor=anchor):
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '#[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_unchecked_source_lifecycle_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_driver.rs'
        original = path.read_text()
        anchor = '    pub(crate) proof fn source_consistency_definition()'
        self.assertIn(anchor, original)
        path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_commit_metadata_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/commit_metadata.rs'
        original = path.read_text()
        for anchor in ['    pub(crate) fn prepare_metadata(', '    pub(crate) fn finish(', '    pub fn signing_bytes(']:
            with self.subTest(anchor=anchor):
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_builder_export_wiring_is_guarded(self):
        path = self.root / 'stockfoil/src/history/local.rs'
        original = path.read_text()
        anchor = 'crate::verified::commit_builder::CommitBuilder'
        self.assertIn(anchor, original)
        path.write_text(original.replace(anchor, 'crate::unchecked::CommitBuilder'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_edit_phases_are_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_edit.rs'
        original = path.read_text()
        for anchor in ['    pub(crate) fn begin_edit<', '    pub(crate) fn finish_edit<']:
            with self.subTest(anchor=anchor):
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_unchecked_hashing_entrypoints_are_rejected(self):
        for file, anchor in [
            ('commit_builder.rs', '    pub fn begin_commit('),
            ('commit_builder.rs', '    pub fn update('),
            ('commit_builder.rs', '    pub fn finish('),
            ('document_commit.rs', '    pub fn commit<'),
            ('materialization_change.rs', '    pub fn change<'),
        ]:
            with self.subTest(file=file, anchor=anchor):
                path = self.root / 'stockfoil/src/verified' / file
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_changed_blake3_adapter_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/blake3_boundary.rs'
        original = path.read_text()
        self.assertIn('self.inner.update(bytes)', original)
        path.write_text(original.replace('self.inner.update(bytes)', 'self.inner.update(&[])'))
        with self.assertRaisesRegex(ValueError, 'Trusted BLAKE3 boundary changed'):
            audit(self.root)

    def test_blake3_dependencies_are_pinned(self):
        for file in BLAKE3_DEPENDENCIES:
            with self.subTest(file=file):
                path = self.root / file
                original = path.read_text()
                path.write_text(original + '\n')
                with self.assertRaisesRegex(ValueError, 'Pinned BLAKE3 dependency changed'):
                    audit(self.root)
                path.write_text(original)

    def test_unchecked_publication_bodies_are_rejected(self):
        for relative, anchor in [
            ('publication.rs', '    pub fn publish_commit('),
            ('publication.rs', '    pub fn commit('),
            ('document_publication.rs', '    pub(crate) fn publish_prepared('),
        ]:
            with self.subTest(anchor=anchor):
                path = self.root / 'stockfoil/src/verified' / relative
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
                with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
                    audit(self.root)
                path.write_text(original)

    def test_publication_wiring_is_guarded(self):
        for relative, anchor in [
            ('stockfoil/src/history/local.rs', 'crate::verified::publication::Prepared'),
            ('stockfoil/src/history/mod.rs', 'crate::verified::publication::Delivered'),
            ('stockfoil/src/meta.rs', 'crate::verified::publication::depth'),
        ]:
            with self.subTest(relative=relative):
                path = self.root / relative
                original = path.read_text()
                self.assertIn(anchor, original)
                path.write_text(original.replace(anchor, 'unchecked_publication'))
                with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
                    audit(self.root)
                path.write_text(original)

    def test_replaced_document_storage_is_rejected(self):
        path = self.root / 'stockfoil/src/document.rs'
        source = path.read_text()
        self.assertIn('crate::verified::document::Document', source)
        path.write_text(source.replace('crate::verified::document::Document', 'crate::unchecked::Document'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_local_acknowledgement_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_local.rs'
        source = path.read_text()
        self.assertIn('    pub fn mark_local(', source)
        path.write_text(source.replace('    pub fn mark_local(', '    #[verifier::external_body]\n    pub fn mark_local('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_commits_since_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/local_queries.rs'
        source = path.read_text()
        self.assertIn('    pub fn commits_since(', source)
        path.write_text(source.replace('    pub fn commits_since(', '    #[verifier::external_body]\n    pub fn commits_since('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_local_metadata_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/local_queries.rs'
        source = path.read_text()
        self.assertIn('    pub fn author_of(', source)
        path.write_text(source.replace('    pub fn author_of(', '    #[verifier::external_body]\n    pub fn author_of('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_draft_type_is_rejected(self):
        path = self.root / 'stockfoil/src/ledger.rs'
        source = path.read_text()
        self.assertIn('crate::verified::draft::Draft', source)
        path.write_text(source.replace('crate::verified::draft::Draft', 'crate::unchecked::Draft'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_removed_local_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        source = path.read_text()
        self.assertIn('pub(crate) mod materialization_local;', source)
        path.write_text(source.replace('pub(crate) mod materialization_local;', ''))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_public_update_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_update.rs'
        source = path.read_text()
        self.assertIn('    pub fn update<', source)
        path.write_text(source.replace('    pub fn update<', '    #[verifier::external_body]\n    pub fn update<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_progress_aggregation_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/progress.rs'
        source = path.read_text()
        self.assertIn('    pub(crate) fn accumulate(', source)
        path.write_text(source.replace('    pub(crate) fn accumulate(', '    #[verifier::external_body]\n    pub(crate) fn accumulate('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_progress_export_is_rejected(self):
        path = self.root / 'stockfoil/src/materialize.rs'
        source = path.read_text()
        self.assertIn('crate::verified::progress::Progress', source)
        path.write_text(source.replace('crate::verified::progress::Progress', 'crate::unchecked::Progress'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_removed_update_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        source = path.read_text()
        self.assertIn('pub(crate) mod materialization_update;', source)
        path.write_text(source.replace('pub(crate) mod materialization_update;', ''))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_step_rounds_are_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_step.rs'
        source = path.read_text()
        self.assertIn('    fn drive_batches<', source)
        path.write_text(source.replace('    fn drive_batches<', '    #[verifier::external_body]\n    fn drive_batches<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_public_step_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_step.rs'
        source = path.read_text()
        self.assertIn('    pub fn step<', source)
        path.write_text(source.replace('    pub fn step<', '    #[verifier::external_body]\n    pub fn step<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_application_call_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_step.rs'
        source = path.read_text()
        self.assertIn('fn apply<M:', source)
        path.write_text(source.replace('fn apply<M:', '#[verifier::external_body]\nfn apply<M:'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_materializer_trait_is_rejected(self):
        path = self.root / 'stockfoil/src/materialize.rs'
        source = path.read_text()
        self.assertIn('crate::verified::materializer::{Materializer, Step}', source)
        path.write_text(source.replace('crate::verified::materializer::{Materializer, Step}', 'crate::unchecked::{Materializer, Step}'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_drive_error_is_rejected(self):
        path = self.root / 'stockfoil/src/drive_error.rs'
        source = path.read_text()
        self.assertIn('crate::verified::drive_error::DriveError', source)
        path.write_text(source.replace('crate::verified::drive_error::DriveError', 'crate::unchecked::DriveError'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_payload_mapping_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/payload_resolution.rs'
        source = path.read_text()
        self.assertIn('    pub fn map_payload<', source)
        path.write_text(source.replace('    pub fn map_payload<', '    #[verifier::external_body]\n    pub fn map_payload<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_document_resolution_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/document.rs'
        source = path.read_text()
        self.assertIn('    pub(crate) fn resolve_batch(', source)
        path.write_text(source.replace('    pub(crate) fn resolve_batch(', '    #[verifier::external_body]\n    pub(crate) fn resolve_batch('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_traversal_export_is_rejected(self):
        path = self.root / 'stockfoil/src/history/traversal.rs'
        path.write_text(path.read_text().replace('crate::verified::change_plan::plan_changes_at', 'crate::unchecked::plan_changes_at'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_bypassed_materialization_planner_is_rejected(self):
        path = self.root / 'stockfoil/src/history/view.rs'
        path.write_text(path.read_text().replace('super::traversal::plan_changes_at', 'super::unchecked::plan_changes_at'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_view_types_are_rejected(self):
        path = self.root / 'stockfoil/src/history/view.rs'
        path.write_text(path.read_text().replace('crate::verified::view', 'crate::unchecked::view'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_bypassed_materialization_emitter_is_rejected(self):
        path = self.root / 'stockfoil/src/history/view.rs'
        path.write_text(path.read_text().replace('crate::verified::change_plan::emit_next_changes', 'crate::unchecked::emit_next_changes'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_public_completion_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_complete.rs'
        source = path.read_text()
        self.assertIn('    pub fn complete<', source)
        path.write_text(source.replace('    pub fn complete<', '    #[verifier::external_body]\n    pub fn complete<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_immutable_batch_export_is_rejected(self):
        path = self.root / 'stockfoil/src/materialize.rs'
        path.write_text(path.read_text().replace('crate::verified::batch_protocol::{Batch, Event}', 'crate::unchecked::{Batch, Event}'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_checkpoint_export_is_rejected(self):
        path = self.root / 'stockfoil/src/history/checkpoint.rs'
        path.write_text(path.read_text().replace('crate::verified::checkpoint::{Checkpoint, Follow}', 'crate::unchecked::{Checkpoint, Follow}'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_public_plan_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_plan.rs'
        source = path.read_text()
        self.assertIn('    pub fn plan<', source)
        path.write_text(source.replace('    pub fn plan<', '    #[verifier::external_body]\n    pub fn plan<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_ledger_storage_is_rejected(self):
        path = self.root / 'stockfoil/src/ledger.rs'
        source = path.read_text()
        self.assertIn('crate::verified::ledger::Ledger', source)
        path.write_text(source.replace('crate::verified::ledger::Ledger', 'crate::unchecked::Ledger'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_policy_equality_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/write_permissions.rs'
        source = path.read_text()
        self.assertIn('    fn eq(&self', source)
        path.write_text(source.replace('    fn eq(&self', '    #[verifier::external_body]\n    fn eq(&self'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_cascade_rejection_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_events.rs'
        source = path.read_text()
        self.assertIn('pub(crate) fn is_rejected(', source)
        path.write_text(source.replace('pub(crate) fn is_rejected(', '#[verifier::external_body]\npub(crate) fn is_rejected('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_completion_counter_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_complete.rs'
        source = path.read_text()
        self.assertIn('fn add_count(', source)
        path.write_text(source.replace('fn add_count(', '#[verifier::external_body]\nfn add_count('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_application_report_types_are_rejected(self):
        path = self.root / 'stockfoil/src/materialize.rs'
        path.write_text(path.read_text().replace('crate::verified::materialization_feedback::{Applied, Rejection}', 'crate::unchecked::{Applied, Rejection}'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_public_current_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_plan.rs'
        source = path.read_text()
        self.assertIn('    pub fn is_current<', source)
        path.write_text(source.replace('    pub fn is_current<', '    #[verifier::external_body]\n    pub fn is_current<'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_ledger_view_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/ledger.rs'
        source = path.read_text()
        self.assertIn('    pub fn view(&self', source)
        path.write_text(source.replace('    pub fn view(&self', '    #[verifier::external_body]\n    pub fn view(&self'))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_unchecked_clock_equality_is_rejected(self):
        path = self.root / 'stockfoil/src/history/clock.rs'
        path.write_text(path.read_text() + '\nimpl Eq for Clock {}\n')
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_history_binding_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_driver.rs'
        original = path.read_text()
        anchor = '    pub(crate) fn check_history('
        self.assertIn(anchor, original)
        path.write_text(original.replace(anchor, '    #[verifier::external_body]\n' + anchor))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_materialization_export_is_rejected(self):
        path = self.root / 'stockfoil/src/materialize.rs'
        path.write_text(path.read_text().replace('crate::verified::materialization_driver::Materialization', 'crate::unchecked::Materialization'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_completion_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        source = path.read_text()
        self.assertIn('pub(crate) mod materialization_complete;', source)
        path.write_text(source.replace('pub(crate) mod materialization_complete;', 'pub(crate) mod unchecked_complete;'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_unchecked_correction_restart_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/materialization_corrections.rs'
        source = path.read_text()
        self.assertIn('    pub(crate) fn restart_from(', source)
        path.write_text(source.replace('    pub(crate) fn restart_from(', '    #[verifier::external_body]\n    pub(crate) fn restart_from('))
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted'):
            audit(self.root)

    def test_replaced_joint_step_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        path.write_text(path.read_text().replace('pub(crate) mod traversal_step;', 'pub(crate) mod unchecked_step;'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_public_iterator_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        path.write_text(path.read_text().replace('pub(crate) mod commits;', 'pub(crate) mod unchecked_commits;'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_changed_range_bound_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('map.range(..limit)', 'map.range(..=limit)'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_replaced_public_history_export_is_rejected(self):
        path = self.root / 'stockfoil/src/history/mod.rs'
        path.write_text(path.read_text().replace('crate::verified::history::History',
            'crate::unverified_history::History'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_public_ancestry_export_is_rejected(self):
        path = self.root / 'stockfoil/src/history/ancestry.rs'
        path.write_text(path.read_text().replace('crate::verified::history::Ancestry',
            'crate::unverified_ancestry::Ancestry'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_replaced_traversal_constructor_module_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/mod.rs'
        path.write_text(path.read_text().replace('pub(crate) mod traversal_init;', 'pub(crate) mod unchecked_init;'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_changed_node_index_representation_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/node_key.rs'
        path.write_text(path.read_text().replace('pub struct NodeIdx(pub(super) u32)', 'pub struct NodeIdx(pub(super) u64)'))
        with self.assertRaisesRegex(ValueError, 'Trusted key dependency changed'):
            audit(self.root)

    def test_replaced_public_iterator_export_is_rejected(self):
        path = self.root / 'stockfoil/src/history/ancestry.rs'
        path.write_text(path.read_text().replace('crate::verified::commits::Commits', 'crate::unchecked::Commits'))
        with self.assertRaisesRegex(ValueError, 'Public API wiring changed'):
            audit(self.root)

    def test_changed_commit_order_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/commit_hash.rs'
        path.write_text(path.read_text().replace('PartialOrd, Ord,', ''))
        with self.assertRaisesRegex(ValueError, 'Trusted key dependency changed'):
            audit(self.root)

    def test_changed_library_contract(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('nodes.sort_unstable();', 'nodes.clear();'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_replaced_fallible_reservation_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('v.try_reserve(additional).is_ok()',
            '{ v.reserve(additional); true }'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_exposed_once_cell_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('cell: OnceLock<T>', 'pub(crate) cell: OnceLock<T>'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_exposed_atomic_cell_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('value: AtomicU64,', 'pub(crate) value: AtomicU64,'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_replaced_static_reference_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('&super::identity::ALLOCATOR',
            'Box::leak(Box::new(super::identity::Allocator::new(1)))'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_changed_atomic_erasure_bridge_is_rejected(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('verus_exec_expr!($body)', 'verus_exec_expr!({})'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_new_assumption_in_application(self):
        path = self.root / 'stockfoil/src/verified/candidates.rs'
        path.write_text(path.read_text() + '\nproof fn unsound() { assume(false); }\n')
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted declaration'):
            audit(self.root)

    def test_changed_key_representation(self):
        for name in ['author_key', 'commit_hash']:
            with self.subTest(name=name):
                path = self.root / f'stockfoil/src/verified/{name}.rs'
                original = path.read_text()
                path.write_text(original.replace('PartialEq, Eq', 'Eq'))
                with self.assertRaisesRegex(ValueError, 'Trusted key dependency changed'):
                    audit(self.root)
                path.write_text(original)

    def test_new_unchecked_application_body(self):
        path = self.root / 'stockfoil/src/verified/candidates.rs'
        path.write_text(path.read_text() + '\n#[verifier::external_body] fn unchecked() {}\n')
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted declaration'):
            audit(self.root)

    def test_import_outside_audit(self):
        path = self.root / 'verification/main.rs'
        path.write_text(path.read_text() + '\n#[path = "../../escape.rs"] mod escape;\n')
        with self.assertRaisesRegex(ValueError, 'outside audited roots'):
            audit(self.root)


if __name__ == '__main__':
    unittest.main()
