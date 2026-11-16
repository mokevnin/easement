# Installation

[Documentation](README.md) · [Project overview](../README.md)

- [Install](#install)

## Install

`easement` runs a SCIP indexer for each indexed language it finds, so install those too, see [Indexers](languages.md#indexers).

```sh
brew install mokevnin/tap/easement
```

Homebrew works on macOS and Linux. Other options:

- the install script for macOS and Linux, which checks the archive against the checksums of the release and installs to `/usr/local/bin`:

  ```sh
  curl -fsSL https://raw.githubusercontent.com/mokevnin/easement/main/install.sh | sh
  ```

  Pass `-b <dir>` to install elsewhere and `-v <version>` to pin a release: `... | sh -s -- -b ~/.local/bin -v 0.1.0`;
- `.deb` and `.rpm` packages for amd64 and arm64 on each [GitHub release](https://github.com/mokevnin/easement/releases): `sudo apt install ./easement_<version>-1_amd64.deb` or `sudo dnf install ./easement-<version>-1.x86_64.rpm`;
- `cargo install easement`;
- prebuilt binaries for macOS, Linux and Windows, with a PowerShell installer for Windows, on each release.

On Windows, Python, PHP, Java and Ruby files cannot be checked, because their indexers do not run there (see [Indexers](languages.md#indexers)): easement checks the other languages and reports those files as uncovered, with the reason.
