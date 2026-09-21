#!/usr/bin/env python3
"""Pure diagnostic contract checks. Does not run an oracle, compiler or GUI."""
import ast
from pathlib import Path
import unittest

from validate_layerprops import NATIVE_TIMEOUT_SECONDS, timeout_diagnostic, timeout_phase


class StartupDiagnosticTests(unittest.TestCase):
    def test_only_fixed_stderr_markers_are_reported(self):
        for payload in (None, b"", "untrusted path/token/assertion text", b"\xff\x00\n"):
            self.assertEqual(timeout_phase(payload), "not_observed")
        for phase in ("entered", "codec", "styles", "view0", "view1", "view2", "view3", "done"):
            for payload in ("LAYERPROPS PHASE " + phase, ("LAYERPROPS PHASE " + phase).encode()):
                self.assertEqual(timeout_phase(payload), phase)
        self.assertEqual(timeout_phase(b"LAYERPROPS PHASE entered\nLAYERPROPS PHASE view3\n"), "view3")

    def test_arbitrary_suffixes_and_prefixes_never_leak(self):
        for line in ("PREFIX LAYERPROPS PHASE entered", "LAYERPROPS PHASE entered secret",
                     "LAYERPROPS PHASE view4", "LAYERPROPS PHASE /private/secret", "LAYERPROPS PHASE \x00"):
            result = timeout_diagnostic(line)
            self.assertEqual(result, "LAYERPROPS TIMEOUT: deadline=30s last_phase=not_observed completion=false")

    def test_done_is_not_completion_and_scanning_is_bounded(self):
        self.assertEqual(NATIVE_TIMEOUT_SECONDS, 30)
        self.assertTrue(timeout_diagnostic(b"LAYERPROPS PHASE done").endswith("completion=false"))
        self.assertEqual(timeout_phase(b"LAYERPROPS PHASE entered\n" + b"x" * (1024 * 1024)), "not_observed")

    def test_timeout_is_reraised_not_retried_or_accepted(self):
        source = Path(__file__).with_name("validate_layerprops.py").read_text()
        tree = ast.parse(source)
        handlers = [node for node in ast.walk(tree) if isinstance(node, ast.ExceptHandler)
                    and isinstance(node.type, ast.Attribute) and node.type.attr == "TimeoutExpired"]
        self.assertEqual(len(handlers), 1)
        self.assertEqual(len(handlers[0].body), 2)  # fixed diagnostic + original raise
        self.assertIsInstance(handlers[0].body[-1], ast.Raise)
        self.assertIsNone(handlers[0].body[-1].exc)
        timed_calls = [node for node in ast.walk(tree) if isinstance(node, ast.Call)
                       and any(k.arg == "timeout" and isinstance(k.value, ast.Name)
                               and k.value.id == "NATIVE_TIMEOUT_SECONDS" for k in node.keywords)]
        self.assertEqual(len(timed_calls), 1)


if __name__ == "__main__":
    unittest.main()
