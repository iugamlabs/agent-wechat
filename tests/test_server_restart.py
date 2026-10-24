"""The server restart loop must survive a child failure with bash errexit enabled."""
from pathlib import Path
import subprocess
import tempfile
import unittest


class ServerRestartTests(unittest.TestCase):
    def test_failed_server_restarts_but_sigterm_exits_without_restarting_wechat(self):
        source = (Path(__file__).resolve().parents[1] / 'docker/entrypoint.sh').read_text()
        loop = source[source.rindex('while true; do'):]
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            server = root / 'server'
            marker = root / 'started'
            server.write_text(f'#!/bin/sh\nif test -f "{marker}"; then exit 143; fi\ntouch "{marker}"\nexit 1\n')
            server.chmod(0o700)
            result = subprocess.run(['bash', '-c', 'set -euo pipefail\n' + loop.replace('/opt/agent-server/agent-server', str(server))],
                                    capture_output=True, text=True, timeout=5)
            self.assertEqual(result.returncode, 0, result.stderr)
            self.assertEqual(result.stdout.count('restarting in 1s'), 1)
