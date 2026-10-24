"""Regression tests for raw and build-masked database credential scanning."""

import hashlib
import importlib.util
import io
from pathlib import Path
import struct
import shutil
import subprocess
import tempfile
import unittest
from unittest.mock import patch


spec = importlib.util.spec_from_file_location(
    'extract_keys', Path(__file__).resolve().parents[1] / 'docker/tools/extract-keys.py'
)
extract = importlib.util.module_from_spec(spec)
spec.loader.exec_module(extract)


class MemoryScanTests(unittest.TestCase):
    def scan_fixture(self, mask=None):
        key = hashlib.sha256(b'synthetic database credential fixture').digest()
        stored = key if mask is None else bytes(a ^ b for a, b in zip(key, mask))
        memory = bytearray(0x12000)
        memory[0x11000:0x11010] = extract.CIPHER_CTX_PATTERN
        # Exercise pointer walking rather than just the direct context scan.
        struct.pack_into('<Q', memory, 0x11020, 0x11800)
        struct.pack_into('<Q', memory, 0x11818, 0x11c00)
        memory[0x11c00:0x11c20] = stored

        def open_fixture(path, *args, **kwargs):
            if path == '/proc/4242/maps':
                return io.StringIO('00010000-00012000 rw-p 00000000 00:00 0\n')
            if path == '/proc/4242/mem':
                return io.BytesIO(memory)
            raise AssertionError(f'Unexpected file access: {path}')

        with patch('builtins.open', side_effect=open_fixture):
            count, candidates = extract.extract_candidates(4242, mask)
        self.assertEqual(count, 1)
        self.assertIn(key.hex(), candidates)
        if mask is not None:
            self.assertNotIn(stored.hex(), candidates)

    def test_existing_build_keeps_raw_candidates(self):
        self.scan_fixture()

    def test_masked_build_resolves_pointer_chain_candidate(self):
        self.scan_fixture(extract.BUILD_PROFILES['ce28c347']['db_xor_mask'])

    def test_arm_masked_build_resolves_pointer_chain_candidate(self):
        self.scan_fixture(extract.BUILD_PROFILES['e9f1cd04']['db_xor_mask'])

    def test_bad_mask_fails_before_process_access(self):
        with self.assertRaises(ValueError):
            extract.extract_candidates(4242, b'short')

    def test_unknown_build_validates_raw_and_known_layouts_without_guessing_image_layout(self):
        with patch.object(extract, 'get_build_id', return_value='unknown'), \
             patch('builtins.print'):
            profile = extract.get_build_profile(4242)
        self.assertTrue(profile['db_page_hmac'])
        self.assertNotIn('image_xor_mask', profile)
        key = hashlib.sha256(b'unknown build core fixture').digest()
        mask = extract.BUILD_PROFILES['ce28c347']['db_xor_mask']
        masked = bytes(a ^ b for a, b in zip(key, mask)).hex()
        candidates = extract.database_candidates([masked], profile)
        self.assertIn(masked, candidates)
        self.assertIn(key.hex(), candidates)

    def test_core_preparation_skips_optional_stores_and_uses_private_output(self):
        with tempfile.TemporaryDirectory() as directory:
            output = Path(directory) / 'output.json'
            databases = [str(Path(directory) / name) for name in ('session.db', 'contact.db', 'message_0.db', 'media_0.db')]
            for path in databases:
                Path(path).write_bytes(b'page')
            with patch('sys.argv', ['extract-keys.py', '--pid', '4242', '--core-only', '--output', str(output)]), \
                 patch.object(extract, 'find_active_account', return_value='test'), \
                 patch.object(extract, 'find_databases', return_value=databases), \
                 patch.object(extract, 'get_build_profile', return_value={}), \
                 patch.object(extract, 'extract_candidates', return_value=(1, [hashlib.sha256(b'core fixture').hexdigest()])), \
                 patch.object(extract, 'test_key', return_value='0') as verify, \
                 patch.object(extract, 'extract_image_keys') as image, \
                 patch('builtins.print'):
                extract.main()
            self.assertEqual(verify.call_count, 3)
            self.assertFalse(image.called)
            self.assertEqual(output.stat().st_mode & 0o777, 0o600)

    def test_core_preparation_reports_missing_stores_before_scanning(self):
        with patch('sys.argv', ['extract-keys.py', '--pid', '4242', '--core-only']), \
             patch.object(extract, 'find_active_account', return_value='test'), \
             patch.object(extract, 'find_databases', return_value=['/test/session.db']), \
             patch.object(extract, 'extract_candidates') as scan, \
             patch('builtins.print'), self.assertRaises(SystemExit):
            extract.main()
        self.assertFalse(scan.called)


@unittest.skipUnless(shutil.which('sqlcipher'), 'SQLCipher CLI required')
class SqlcipherPageTests(unittest.TestCase):
    def test_page_check_against_real_sqlcipher_database(self):
        key = hashlib.sha256(b'synthetic SQLCipher integration fixture').hexdigest()
        with tempfile.TemporaryDirectory() as directory:
            database = Path(directory) / 'fixture.db'
            subprocess.run(
                ['sqlcipher', str(database)],
                input=f"PRAGMA key = \"x'{key}'\";\nPRAGMA cipher_compatibility=4;\nCREATE TABLE fixture(value TEXT);\nINSERT INTO fixture VALUES ('test');\n",
                text=True, capture_output=True, check=True,
            )
            page = database.read_bytes()[:4096]
            self.assertTrue(extract.matches_sqlcipher4_page(key, page))
            self.assertFalse(extract.matches_sqlcipher4_page('00' * 32, page))
            modified = bytearray(page)
            modified[80] ^= 1
            self.assertFalse(extract.matches_sqlcipher4_page(key, bytes(modified)))
            self.assertFalse(extract.matches_sqlcipher4_page(key, page[:100]))
            self.assertEqual(extract.test_key(str(database), key), '1')


if __name__ == '__main__':
    unittest.main()
