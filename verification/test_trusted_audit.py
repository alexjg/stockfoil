#!/usr/bin/env python3
"""Regression checks for the trust-boundary guardrail."""
import shutil
import tempfile
import unittest
from pathlib import Path
from check_trusted import ROOT, audit


class TrustAudit(unittest.TestCase):
    def setUp(self):
        self.directory = tempfile.TemporaryDirectory()
        self.addCleanup(self.directory.cleanup)
        self.root = Path(self.directory.name)
        for name in ['verification', 'stockfoil/src/verified']:
            shutil.copytree(ROOT / name, self.root / name)

    def test_current_manifest(self):
        self.assertEqual(audit(self.root), 2)

    def test_changed_library_contract(self):
        path = self.root / 'stockfoil/src/verified/stdlib.rs'
        path.write_text(path.read_text().replace('nodes.sort_unstable();', 'nodes.clear();'))
        with self.assertRaisesRegex(ValueError, 'Trusted source changed'):
            audit(self.root)

    def test_new_assumption_in_application(self):
        path = self.root / 'stockfoil/src/verified/candidates.rs'
        path.write_text(path.read_text() + '\nproof fn unsound() { assume(false); }\n')
        with self.assertRaisesRegex(ValueError, 'Unlisted trusted declaration'):
            audit(self.root)

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
