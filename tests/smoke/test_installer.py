"""Portable tests for the repository shell installer (install.sh).

Runs install.sh against a local HTTP server serving fake release trees built
from tests/fixtures/installer/. No real GitHub API access is required.

Coverage: platform detection (x86_64/aarch64, unsupported OS/arch), version
resolution (explicit pin with/without leading v, latest), failed download,
checksum mismatch, missing checksum, custom install dir, PATH guidance,
cleanup on failure, and atomic replacement of an existing binary.
"""

import functools
import hashlib
import http.server
import io
import os
import pathlib
import subprocess
import tarfile
import tempfile
import threading

ROOT = pathlib.Path(__file__).resolve().parents[2]
INSTALL_SH = ROOT / "install.sh"
FIXTURES = ROOT / "tests" / "fixtures" / "installer"
STUB = (FIXTURES / "chronicle-stub").read_bytes()

TARGETS = ("x86_64-unknown-linux-gnu", "aarch64-unknown-linux-gnu")
VERSION = "0.1.0"


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as f:
        for chunk in iter(lambda: f.read(65536), b""):
            h.update(chunk)
    return h.hexdigest()


def make_archive(path, binary=STUB):
    """tar.gz with a single 'chronicle' binary at the top level."""
    with tarfile.open(path, "w:gz") as tf:
        info = tarfile.TarInfo("chronicle")
        info.size = len(binary)
        info.mode = 0o755
        tf.addfile(info, io.BytesIO(binary))


def build_release(
    root,
    version=VERSION,
    targets=TARGETS,
    skip_archive=None,
    corrupt=None,
    missing_lines=None,
):
    """Create <root>/v<version>/ containing archives + SHA256SUMS."""
    tag = "v" + version
    d = root / tag
    d.mkdir(parents=True, exist_ok=True)
    lines = []
    for t in targets:
        name = "chronicle-{}-{}.tar.gz".format(version, t)
        if skip_archive == name:
            continue
        make_archive(d / name)
        digest = sha256_file(d / name)
        if corrupt == name:
            digest = "0" * 64
        lines.append("{}  {}".format(digest, name))
    if missing_lines is not None:
        lines = [line for line in lines if not any(m in line for m in missing_lines)]
    (d / "SHA256SUMS").write_text("\n".join(lines) + "\n")
    return d


class _QuietHandler(http.server.SimpleHTTPRequestHandler):
    def log_message(self, format, *args):
        pass


def serve(root):
    handler = functools.partial(_QuietHandler, directory=str(root))
    httpd = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    base = "http://127.0.0.1:{}".format(httpd.server_address[1])
    return httpd, base


def run_installer(
    root,
    base,
    *,
    install_dir=None,
    version=None,
    os_name="Linux",
    machine="x86_64",
    path=None,
    tmpdir=None,
    home=None,
    preexisting=None,
):
    env = dict(os.environ)
    env.update(
        {
            "CHRONICLE_API_URL": base + "/latest.json",
            "CHRONICLE_BASE_URL": base,
            "CHRONICLE_UNAME_OS": os_name,
            "CHRONICLE_UNAME_MACHINE": machine,
            "HOME": str(home or root / "home"),
        }
    )
    if install_dir is not None:
        env["CHRONICLE_INSTALL_DIR"] = str(install_dir)
    if version is not None:
        env["CHRONICLE_VERSION"] = version
    if path is not None:
        env["PATH"] = path
    if tmpdir is not None:
        env["TMPDIR"] = str(tmpdir)
    if preexisting is not None:
        p = install_dir or (root / "home" / ".local" / "bin")
        p.mkdir(parents=True, exist_ok=True)
        (p / "chronicle").write_text(preexisting)
    proc = subprocess.run(
        ["sh", str(INSTALL_SH)],
        env=env,
        capture_output=True,
        text=True,
        cwd=str(root),
    )
    return proc


def _with_server(tmp, build=True):
    root = tmp / "site"
    root.mkdir()
    if build:
        build_release(root)
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
    httpd, base = serve(root)
    return httpd, base


# --- platform detection -----------------------------------------------------


def test_linux_x86_64_asset_selection():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, machine="x86_64")
            assert proc.returncode == 0, proc.stderr
            assert (
                "chronicle-{}-x86_64-unknown-linux-gnu.tar.gz".format(VERSION)
                in proc.stdout
            )
            installed = tmp / "home" / ".local" / "bin" / "chronicle"
            assert installed.read_bytes() == STUB
            assert os.access(installed, os.X_OK)
        finally:
            httpd.shutdown()


def test_linux_aarch64_asset_selection():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, machine="aarch64")
            assert proc.returncode == 0, proc.stderr
            assert (
                "chronicle-{}-aarch64-unknown-linux-gnu.tar.gz".format(VERSION)
                in proc.stdout
            )
        finally:
            httpd.shutdown()


def test_unsupported_os():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, os_name="Darwin", machine="x86_64")
            assert proc.returncode != 0
            assert "unsupported operating system" in proc.stderr
        finally:
            httpd.shutdown()


def test_unsupported_architecture():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, machine="i686")
            assert proc.returncode != 0
            assert "unsupported architecture" in proc.stderr
        finally:
            httpd.shutdown()


# --- version resolution -----------------------------------------------------


def test_explicit_version_with_and_without_v():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        build_release(root, version="0.1.0")
        build_release(root, version="0.2.0")
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, version="v0.1.0")
            assert proc.returncode == 0, proc.stderr
            assert "chronicle-0.1.0-" in proc.stdout
            proc = run_installer(tmp, base, version="0.2.0")
            assert proc.returncode == 0, proc.stderr
            assert "chronicle-0.2.0-" in proc.stdout
        finally:
            httpd.shutdown()


def test_latest_release_selection():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        build_release(root, version="0.1.0")
        build_release(root, version="0.2.0")
        (root / "latest.json").write_text('{"tag_name": "v0.2.0"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base)
            assert proc.returncode == 0, proc.stderr
            assert "chronicle-0.2.0-" in proc.stdout
        finally:
            httpd.shutdown()


def test_missing_release_fails():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, version="v9.9.9")
            assert proc.returncode != 0
            assert "failed to download" in proc.stderr
        finally:
            httpd.shutdown()


# --- download and integrity -------------------------------------------------


def test_failed_download():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        build_release(
            root,
            skip_archive="chronicle-{}-x86_64-unknown-linux-gnu.tar.gz".format(VERSION),
        )
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, machine="x86_64")
            assert proc.returncode != 0
            assert "failed to download" in proc.stderr
            assert not (tmp / "home" / ".local" / "bin" / "chronicle").exists()
        finally:
            httpd.shutdown()


def test_checksum_mismatch():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        build_release(
            root, corrupt="chronicle-{}-x86_64-unknown-linux-gnu.tar.gz".format(VERSION)
        )
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, machine="x86_64")
            assert proc.returncode != 0
            assert "checksum mismatch" in proc.stderr
            assert not (tmp / "home" / ".local" / "bin" / "chronicle").exists()
        finally:
            httpd.shutdown()


def test_missing_checksum_line():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        build_release(root, missing_lines=["x86_64-unknown-linux-gnu"])
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, machine="x86_64")
            assert proc.returncode != 0
            assert "no SHA-256 checksum found" in proc.stderr
        finally:
            httpd.shutdown()


def test_missing_checksums_file():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        d = build_release(root)
        (d / "SHA256SUMS").unlink()
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, machine="x86_64")
            assert proc.returncode != 0
            assert "failed to download checksums" in proc.stderr
        finally:
            httpd.shutdown()


# --- installation destination ----------------------------------------------


def test_custom_install_dir():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            target = tmp / "custom" / "bin"
            proc = run_installer(tmp, base, install_dir=target, machine="x86_64")
            assert proc.returncode == 0, proc.stderr
            assert (target / "chronicle").read_bytes() == STUB
        finally:
            httpd.shutdown()


def test_install_dir_not_on_path_prints_instruction():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            target = tmp / "offpath"
            proc = run_installer(
                tmp, base, install_dir=target, machine="x86_64", path="/usr/bin:/bin"
            )
            assert proc.returncode == 0, proc.stderr
            assert "not on your PATH" in proc.stderr
            assert 'export PATH="{}:$PATH"'.format(target) in proc.stderr
        finally:
            httpd.shutdown()


def test_install_dir_on_path_no_instruction():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        httpd, base = _with_server(tmp)
        try:
            target = tmp / "onpath"
            target.mkdir(parents=True)
            proc = run_installer(
                tmp,
                base,
                install_dir=target,
                machine="x86_64",
                path="{}:/usr/bin:/bin".format(target),
            )
            assert proc.returncode == 0, proc.stderr
            assert "not on your PATH" not in proc.stderr
        finally:
            httpd.shutdown()


# --- cleanup and atomic replace --------------------------------------------


def test_cleanup_on_failure_and_success():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        tmpdir = tmp / "scratch"
        tmpdir.mkdir()
        httpd, base = _with_server(tmp)
        try:
            proc = run_installer(tmp, base, machine="x86_64", tmpdir=tmpdir)
            assert proc.returncode == 0, proc.stderr
            assert list(tmpdir.iterdir()) == [], "temp residue after success"
        finally:
            httpd.shutdown()
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        tmpdir = tmp / "scratch"
        tmpdir.mkdir()
        root = tmp / "site"
        root.mkdir()
        build_release(
            root, corrupt="chronicle-{}-x86_64-unknown-linux-gnu.tar.gz".format(VERSION)
        )
        (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
        httpd, base = serve(root)
        try:
            proc = run_installer(tmp, base, machine="x86_64", tmpdir=tmpdir)
            assert proc.returncode != 0
            assert list(tmpdir.iterdir()) == [], "temp residue after failure"
        finally:
            httpd.shutdown()


def test_existing_binary_replaced_only_after_verification():
    with tempfile.TemporaryDirectory() as td:
        tmp = pathlib.Path(td)
        root = tmp / "site"
        root.mkdir()
        httpd, base = serve(root)
        try:
            target = tmp / "bin"
            # Failed install leaves the old binary intact.
            build_release(
                root,
                corrupt="chronicle-{}-x86_64-unknown-linux-gnu.tar.gz".format(VERSION),
            )
            (root / "latest.json").write_text('{"tag_name": "v' + VERSION + '"}')
            proc = run_installer(
                tmp,
                base,
                install_dir=target,
                machine="x86_64",
                preexisting="old-version",
            )
            assert proc.returncode != 0
            assert (target / "chronicle").read_text() == "old-version"
            assert not list(
                target.glob(".chronicle.tmp.*")
            ), "partial temp binary left behind"
            # Successful install replaces it.
            build_release(root)
            proc = run_installer(
                tmp,
                base,
                install_dir=target,
                machine="x86_64",
                preexisting="old-version",
            )
            assert proc.returncode == 0, proc.stderr
            assert (target / "chronicle").read_bytes() == STUB
        finally:
            httpd.shutdown()


if __name__ == "__main__":
    import unittest

    suite = unittest.TestSuite()
    for name, fn in sorted(globals().items()):
        if name.startswith("test_") and callable(fn):
            suite.addTest(unittest.FunctionTestCase(fn))
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    raise SystemExit(0 if result.wasSuccessful() else 1)
