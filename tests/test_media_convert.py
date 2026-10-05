import importlib.util
from pathlib import Path
import shutil
import struct
import subprocess
import unittest
from unittest.mock import patch

ROOT = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location('media_convert', ROOT / 'docker/tools/media-convert.py')
convert = importlib.util.module_from_spec(spec)
spec.loader.exec_module(convert)
JPEG = (ROOT / 'tests/fixtures/synthetic.jpg').read_bytes()


def encrypted_payload(data, key=0x63, xor_size=None, budget_ms=3000):
    xor_size = len(data) - 32 if xor_size is None else xor_size
    encrypted = data[:-xor_size] + bytes(b ^ key for b in data[-xor_size:])
    return struct.pack('<II', xor_size, budget_ms) + encrypted


class JpegStructure(unittest.TestCase):
    def test_metadata_and_trailer_markers_do_not_define_eoi(self):
        # An APP marker contains a fake EOI. The real EOI precedes another fake in the trailer.
        app = b'\xff\xe1\x00\x08AA\xff\xd9BB'
        image = JPEG[:2] + app + JPEG[2:]
        self.assertEqual(convert.normalize_jpeg(image + b'XX\xff\xd9YY'), image)

    def test_truncated_segments_and_early_eoi_fail_before_decode(self):
        for data in (JPEG[:-2], b'\xff\xd8\xff\xd9',
                     JPEG[:2] + b'\xff\xe1\x00\x01' + JPEG[2:],
                     JPEG[:2] + b'\xff\xe1\xff\xff' + JPEG[2:]):
            with self.subTest(size=len(data)), patch.object(convert, 'decode_image') as decoder:
                with self.assertRaises(ValueError):
                    convert.normalize_image(data)
                decoder.assert_not_called()

    def test_trailer_scan_and_candidate_limits(self):
        with self.assertRaises(ValueError):
            convert.normalize_jpeg(JPEG + b'X' * (convert.MAX_JPEG_TRAILING_BYTES + 1))
        key = 0x63
        payload = encrypted_payload(JPEG + b'X' * (convert.MAX_JPEG_TRAILING_BYTES + 1))
        self.assertNotIn(key, convert.jpeg_xor_candidates(payload[8:], len(payload) - 8 - 32))
        # Distinct encrypted EOI pairs for more keys than the permitted count.
        noisy = b''.join(bytes((0xff ^ k, 0xd9 ^ k)) for k in range(convert.MAX_XOR_CANDIDATES + 1))
        with self.assertRaises(ValueError):
            convert.jpeg_xor_candidates(noisy, len(noisy))

    def test_eoi_outside_xor_region_cannot_establish_a_key(self):
        data = JPEG + b'\x10\x36\x20\x06XX'
        with patch.object(convert, 'decode_image') as decoder, self.assertRaises(ValueError):
            convert.recover_jpeg_xor(struct.pack('<II', 6, 3000) + data)
        decoder.assert_not_called()

    def test_ambiguous_validated_candidates_are_rejected(self):
        # Simulate two independently decoded candidates to exercise the uniqueness rule.
        with patch.object(convert, 'jpeg_xor_candidates', return_value=[1, 2]), \
             patch.object(convert, 'normalize_jpeg', side_effect=lambda data: data), \
             patch.object(convert, 'decode_image') as decoder, self.assertRaisesRegex(ValueError, 'Ambiguous'):
            convert.recover_jpeg_xor(struct.pack('<II', 6, 3000) + b'prefixXXsuffix')
        self.assertEqual(decoder.call_count, 2)

    def test_budget_expiration_does_not_return_a_partial_choice(self):
        with patch.object(convert, 'decode_image') as decoder, self.assertRaises(ValueError):
            convert.recover_jpeg_xor(encrypted_payload(JPEG + b'X' * 24, budget_ms=0))
        decoder.assert_not_called()


@unittest.skipUnless(shutil.which('ffmpeg'), 'requires the runtime FFmpeg codec')
class RuntimeCodec(unittest.TestCase):
    def test_jpeg_normalization_and_recovery(self):
        for count in (0, 24, 191):
            with self.subTest(trailing=count):
                image = JPEG + b'X' * count
                self.assertEqual(convert.normalize_image(image), (JPEG, 'jpeg'))
                self.assertEqual(convert.validate_image(image), (b'ok', 'validation'))
                for key in (0, 0x63, 0xff):
                    self.assertEqual(convert.recover_jpeg_xor(encrypted_payload(image, key)), (bytes([key]), 'xor'))

    def test_truncated_entropy_with_forged_eoi_is_rejected(self):
        sos = JPEG.index(b'\xff\xda')
        scan_start = sos + 2 + int.from_bytes(JPEG[sos + 2:sos + 4], 'big')
        for cut in (scan_start, scan_start + 10, len(JPEG) - 100):
            with self.subTest(cut=cut), self.assertRaises(ValueError):
                convert.normalize_image(JPEG[:cut] + b'\xff\xd9')

    def test_wrong_xor_and_corruption_fail(self):
        payload = encrypted_payload(JPEG + b'X' * 24)
        encrypted = payload[8:]
        size = struct.unpack('<I', payload[:4])[0]
        wrong = encrypted[:-size] + bytes(b ^ 0x62 for b in encrypted[-size:])
        with self.assertRaises(ValueError):
            convert.normalize_image(wrong)
        # Valid JPEG structure cannot rescue damaged pixels.
        damaged = JPEG[:-100] + b'\xff\xd9' + b'X' * 24
        with self.assertRaises(ValueError):
            convert.recover_jpeg_xor(encrypted_payload(damaged))

    def test_other_formats_keep_validation_and_bytes(self):
        for fmt in ('png', 'gif', 'webp'):
            data = (ROOT / f'tests/fixtures/synthetic.{fmt}').read_bytes()
            with self.subTest(format=fmt):
                self.assertEqual(convert.normalize_image(data), (data, fmt))
                with self.assertRaises(ValueError):
                    convert.normalize_image(data[:-1])

    def test_command_returns_normalized_bytes(self):
        result = subprocess.run(['python3', str(ROOT / 'docker/tools/media-convert.py'), 'normalize-image'],
                                input=JPEG + b'X' * 191, capture_output=True, timeout=10)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertEqual(result.stdout, JPEG)
        self.assertIn(b'FORMAT:jpeg', result.stderr)

    def test_wxgf_partition_and_conversion_still_produce_valid_image(self):
        hevc = subprocess.run(['ffmpeg', '-hide_banner', '-loglevel', 'error', '-f', 'lavfi',
                               '-i', 'color=size=32x32:rate=1', '-frames:v', '1', '-c:v', 'libx265',
                               '-threads', '1', '-x265-params', 'pools=none:frame-threads=1', '-f', 'hevc', '-'],
                              capture_output=True, timeout=10)
        if hevc.returncode:
            self.skipTest('FFmpeg lacks libx265')
        data = b'wxgf' + bytes([5]) + len(hevc.stdout).to_bytes(4, 'big') + hevc.stdout
        result, fmt = convert.wxgf2img(data)
        self.assertEqual(fmt, 'jpeg')
        self.assertEqual(convert.normalize_image(result), (result, fmt))


if __name__ == '__main__':
    unittest.main()
