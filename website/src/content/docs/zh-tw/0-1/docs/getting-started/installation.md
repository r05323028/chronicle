---
title: 安裝
description: 安裝 Chronicle 發行版二進位檔，或從原始碼建置工作區。
slug: zh-tw/0-1/docs/getting-started/installation
---

## 建議：發行版安裝器

發行版二進位檔支援 Linux `x86_64` 與 `aarch64`／`arm64`。安裝器會解析最新穩定版、選擇對應封存檔、驗證 `SHA256SUMS`，驗證成功後才安裝。

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh | sh
```

預設安裝到 `$HOME/.local/bin`，不會修改 shell 設定。完成後執行：

```bash
chronicle --version
chronicle doctor
```

固定版本：

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_VERSION=v0.1.0 sh
```

指定目錄：

```bash
curl -fsSL https://raw.githubusercontent.com/r05323028/chronicle/main/install.sh \
  | CHRONICLE_INSTALL_DIR=/some/path sh
```

## 從原始碼建置

```bash
git clone https://github.com/r05323028/chronicle
cd chronicle
cargo build --release --locked
```

Linux 建置會包含已提交的 eBPF 捕捉物件；其他平台提供列出、檢查、重播規劃與驗證、doctor 及 fixture 錄製，但不提供 live capture。

:::note
Docker、Kubernetes、PostgreSQL 與 S3 儲存目前尚未實作。
:::
