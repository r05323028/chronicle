---
title: 安裝
description: 安裝 Chronicle 發行版本的二進位檔，或從原始碼建置工作區。
slug: zh-tw/0-1/docs/getting-started/installation
---

## 建議：安裝發行版本

Chronicle 發行版本的二進位檔支援 Linux `x86_64` 與 `aarch64`／`arm64`。儲存庫提供的安裝器會取得 GitHub Release 的最新穩定版本、選擇相符的封存檔、驗證 `SHA256SUMS`，只有驗證成功後才會安裝。

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

預設安裝到 `$HOME/.local/bin`。腳本不會修改 shell 設定；如果該目錄不在 `PATH` 中，腳本會輸出應該執行的 `export PATH=...` 指令。

確認二進位檔與主機狀態：

```bash
chronicle --version
chronicle doctor
```

`doctor` 不會修改系統。它會回報平台、架構、cgroup v2、BTF、內嵌擷取程式、掛接狀態、能力、WAL／輸出、協定與 replay policy 的就緒狀態，並提供修正建議。

### 固定發行版本

版本可以包含開頭的 `v`，也可以省略：

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

### 指定安裝目錄

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

## 手動安裝發行版本

安裝器無法執行時，請使用這個方式：

1. 從 GitHub Release 下載相符的 `chronicle-<version>-<target>.tar.gz` 與 `SHA256SUMS`。
2. 解壓縮前先驗證封存檔。

   ```bash
   sha256sum -c SHA256SUMS
   ```

3. 解壓縮封存檔，並將頂層的 `chronicle` 二進位檔放到 `PATH` 中。
4. 執行 `chronicle --version` 與 `chronicle doctor`。

發行流程會提供 `x86_64-unknown-linux-gnu` 與 `aarch64-unknown-linux-gnu` 封存檔。其他平台請勿自行猜測 target 名稱。

## 從原始碼建置

工作區會在 `rust-toolchain.toml` 中固定 Rust 工具鏈：

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

Linux 建置會包含已提交到儲存庫的 eBPF 擷取物件。其他平台提供不含 live capture 的可攜功能：列出、檢查、重播規劃與驗證、doctor，以及 fixture 錄製。

只有 eBPF pipeline 開發需要使用儲存庫 README 所述的獨立 nightly 重建流程。建置後執行 `chronicle doctor`，查看目前主機支援哪些功能。

## live capture 的主機需求

- Linux 6.1 或更新版本。
- 啟用 cgroup v2。
- `/sys/kernel/btf/vmlinux` 提供 BTF。
- 小端序 `x86_64` 或 `aarch64`。
- 錄製程序需要 `CAP_BPF` 與 `CAP_NET_ADMIN`。
- 二進位檔中包含內嵌的擷取程式。

應用程式必須提供明文 HTTP/1.1 流量。目前的擷取路徑無法解讀 TLS 密文。

:::note
Docker 與 Kubernetes 封裝尚未實作。Chronicle 目前發佈本機檔案系統 artifact；PostgreSQL 與 S3 相容的持久化屬於未來工作。
:::
