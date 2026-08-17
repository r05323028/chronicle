---
title: Installation
description: Install the released Chronicle binary or build the workspace from source.
slug: 0-1/docs/getting-started/installation
---

## Build from source (currently usable)

A public release does not exist yet, so building from source is the currently usable installation path. The release installer below becomes the recommended path starting with the first public release.

## Release installer (from the first public release)

Chronicle release binaries will be supported on Linux `x86_64` and `aarch64`/`arm64`. The repository installer will resolve the configured stable GitHub Release, select the matching archive, verify `SHA256SUMS`, and install only after verification succeeds.

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

The default destination is `$HOME/.local/bin`. The script does not edit shell configuration. If the directory is not on `PATH`, it prints an `export PATH=...` instruction.

Verify the binary and host:

```bash
chronicle --version
chronicle doctor
```

`doctor` is non-destructive. It reports platform, architecture, cgroup v2, BTF, embedded capture programs, attachment, capabilities, WAL/output, protocol, and replay-policy readiness with remediation.

### Pin a release

The version may include or omit the leading `v`:

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

### Choose an install directory

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

## Manual release installation

Use this path when the installer cannot run:

1. Download the matching `chronicle-<version>-<target>.tar.gz` and `SHA256SUMS` files from a GitHub Release.

2. Verify the archive before extraction.

   ```bash
   sha256sum -c SHA256SUMS
   ```

3. Extract the archive and put the top-level `chronicle` binary on `PATH`.

4. Run `chronicle --version` and `chronicle doctor`.

The release workflow publishes `x86_64-unknown-linux-gnu` and `aarch64-unknown-linux-gnu` archives. Do not guess a target name for another platform.

## Build from source

The workspace pins its Rust toolchain in `rust-toolchain.toml`:

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

Linux builds include the checked-in eBPF capture object. Other platforms build the portable surface: listing, inspection, replay planning and verification, doctor, and fixture recording, without live capture.

Only eBPF pipeline development needs the separate nightly rebuild described in the repository README. Run `chronicle doctor` after building to see what the current host supports.

## Host requirements for live capture

* Linux 6.1 or newer.
* cgroup v2 enabled.
* BTF available at `/sys/kernel/btf/vmlinux`.
* Little-endian `x86_64` or `aarch64`.
* `CAP_BPF` and `CAP_NET_ADMIN` for the recording process.
* Embedded capture programs present in the binary.

The application must expose plaintext HTTP/1.1 traffic. TLS ciphertext is opaque to the current capture path.

:::note
Docker and Kubernetes packaging are not implemented. Chronicle currently publishes local filesystem artifacts; PostgreSQL and S3-compatible persistence are future work.
:::
