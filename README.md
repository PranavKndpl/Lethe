# Lethe

> **"The River of Forgetfulness"**

**Lethe** is a serverless, user-space, distributed encrypted filesystem designed for **plausible deniability** and high resilience.

Unlike traditional encryption tools (e.g., VeraCrypt, BitLocker) that create a single monolithic container file, Lethe shards your data into thousands of small, encrypted blocks that resemble random noise or temporary cache files. This architecture makes it difficult for forensic analysis to determine the true size or nature of the stored data.

---

## 🌟 Key Features

* **🛡️ Zero Knowledge Architecture:** All data is encrypted client-side using **XChaCha20-Poly1305** before it ever touches the disk. Keys are derived using **Argon2id**.
* **🧩 Distributed Storage:** Data is split into 64KB shards (`blk_uuid.bin`). Losing one block does not corrupt the entire vault, only the specific file associated with it.
* **👻 Plausible Deniability:** The vault looks like a folder of garbage data. There are no file headers or signatures identifying it as a Lethe volume.
* **⚡ Serverless & Lightweight:** No background services or drivers required. The filesystem lives only in RAM while mounted.
* **🌍 Cross-Platform:**
* **Windows:** Uses a custom high-performance WebDAV driver.
* **Linux / macOS:** Uses native FUSE (Filesystem in Userspace) for maximum speed.

---

## 🧠 Architecture Details

Lethe uses a **Split-Stack Architecture**:

1. **Storage Layer (`lethe_core`):**
* **Encryption:** XChaCha20-Poly1305 (Authenticated Encryption).
* **Key Derivation:** Argon2id (Resistant to GPU cracking).
* **Compression:** Zstd (Level 3) applied before encryption to maximize entropy.
* **Indexing:** Metadata is stored in `meta_X.bin` replicas, serialized with CBOR.


2. **Interface Layer (`lethe_cli`):**
* **Windows:** Implements a custom **WebDAV** server (`dav-server`, `warp`) that loops back to `127.0.0.1:4918`. Windows Explorer treats this as a Network Drive.
* **Unix:** Implements a **FUSE** filesystem (`fuser`) that translates kernel file operations directly to Lethe's block storage logic.


---

## 📥 Installation

### Windows

1. Download the latest release (`Lethe v1.0.zip`) from the [Releases Page](https://github.com/PranavKndpl/Lethe/releases/tag/v1.0).
2. Open the `Windows` folder.
3. Run `LetheInstaller.exe`. This will install the program and add `lethe` to your global PATH.
4. Open PowerShell or Command Prompt and type `lethe --help` or `lethe --version` to verify.

### Linux

**Prerequisites:** Ensure FUSE 2 compatibility is installed (Ubuntu 22.04+ defaults to FUSE 3).

```bash
sudo apt-get install libfuse2

```

**Install:**

1. Download and extract the release zip.
2. Open the `unix` folder in your terminal.
3. Run the installer:
```bash
sudo ./install.sh

```


*(This automatically detects your OS, moves the correct binary to `/usr/local/bin`, and sets permissions.)*

### macOS

**Prerequisites:** You must install **[macFUSE](https://www.google.com/search?q=https://osxfuse.github.io/)** to allow mounting custom drives.

**Install:**

1. Download and extract the release zip.
2. Open the `unix` folder in your terminal.
3. Run the installer:
```bash
sudo ./install.sh

```



---

## 🚀 Quick Start Guide

### 1. Initialize a Vault

Create a new secure vault. By default, this creates a hidden folder at `~/.lethe_vault`.

```bash
lethe init
# You will be prompted to set a secure password.

```

*Optional:* Create a vault in a specific location (e.g., a USB drive):

```bash
lethe init --path "D:/MySecretVault"

```

### 2. Unlock & Mount

Mount your vault to access files.

* **Windows:** Mounts automatically to Drive **Z:**.
* **Linux/Mac:** Mounts to `~/LetheMount` (or a desktop Volume on macOS).

```bash
lethe mount

```

*Optional:* Mount a specific vault to a specific drive/folder:

```bash
lethe mount --vault "D:/MySecretVault" --mountpoint "X:"

```

### 3. Use Your Files

Once mounted, use the drive normally!

* Copy/Paste files via Explorer/Finder.
* Edit documents directly.
* Watch videos or view images.

Everything you write is encrypted on-the-fly in RAM and saved as sharded blocks to the vault folder.

### 4. Lock & Dismount

To close the vault, simply go to the terminal where Lethe is running and press:
**`Ctrl + C`**

This will:

1. Flush any remaining data to disk.
2. Unmount the virtual drive.
3. Wipe the encryption keys from RAM.

---

## 🛠️ Advanced Usage

### Garbage Collection

Because Lethe is designed for speed, deleting a file inside the mount point removes it from the index but leaves the encrypted blocks on disk (as "orphans"). To free up space, run the cleaner:

```bash
lethe clean

```

### Manual File Management

You can move files into the vault without mounting it using the CLI:

```bash
# Upload a file
lethe put --file "./document.pdf" --dest "/docs/document.pdf"

# Download a file
lethe get --src "/docs/document.pdf" --out "./restored.pdf"

# List files
lethe ls

```

---

## 🏗️ Building from Source

If you prefer to build Lethe yourself, you will need **Rust** installed.

1. **Clone the repository:**
```bash
git clone https://github.com/YourUsername/Lethe.git
cd Lethe

```


2. **Install Dependencies:**
* **Linux:** `sudo apt install libfuse-dev pkg-config`
* **macOS:** `brew install macfuse`
* **Windows:** No extra dependencies needed.


3. **Build:**
```bash
cargo build --release

```



The binary will be located in `target/release/lethe_cli`.

---


## ⚠️ Disclaimer

**Lethe is provided "as is", without warranty of any kind.**
While Lethe uses industry-standard cryptographic primitives (Argon2, ChaCha20, Poly1305), it is experimental software. **Always keep backups of critical data.**

* **Don't** delete the `.lethe_vault` folder unless you want to destroy your data.
* **Don't** rename the `blk_*.bin` files inside the vault manually.

---

### License

Distributed under the MIT License. See `LICENSE` for more information.
