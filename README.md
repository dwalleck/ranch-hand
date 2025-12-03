# ranch-hand

A CLI tool for managing Rancher Desktop.

## Features

- **Backend Control**: Start, stop, restart, and check status of Rancher Desktop
- **Settings Management**: View and modify settings using dot notation paths
- **k3s Cache Management**: List and pre-populate k3s version cache
- **Network Diagnostics**: Comprehensive connectivity and certificate checks
- **Direct API Access**: Interact with the Rancher Desktop HTTP API

## Installation

### From Releases

Download the latest release for your platform from the [Releases page](https://github.com/dwalleck/ranch-hand/releases).

**Linux/macOS:**
```bash
# Download and extract (example for Linux x64)
curl -LO https://github.com/dwalleck/ranch-hand/releases/latest/download/rh-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
tar -xzf rh-v0.1.0-x86_64-unknown-linux-gnu.tar.gz
sudo mv rh /usr/local/bin/

# Verify installation
rh version
```

**Windows:**
Download the `.zip` file, extract `rh.exe`, and add it to your PATH.

### From Source

Requires Rust 1.70 or later.

```bash
git clone https://github.com/dwalleck/ranch-hand.git
cd ranch-hand
cargo install --path .
```

## Usage

### Backend Control

```bash
# Check backend status
rh status

# Start/stop/restart the backend
rh start
rh stop
rh restart
```

### Settings Management

```bash
# Show all settings
rh settings

# Get a specific setting
rh settings get kubernetes.version
rh settings get containerEngine.name

# Set a setting value
rh settings set kubernetes.enabled true
rh settings set containerEngine.name containerd

# Factory reset
rh settings reset
```

### k3s Cache Management

```bash
# List cached versions
rh cache list

# Pre-populate cache for a specific version
rh cache populate v1.33.3+k3s1
```

### Network Diagnostics

```bash
# Run comprehensive diagnostics
rh diagnose

# Check SSL certificates for required domains
rh certs check
```

### Direct API Access

```bash
# GET request
rh api /v1/settings

# PUT request with body
rh api /v1/settings -m PUT -b '{"kubernetes": {"enabled": true}}'

# POST with body from file
rh api /v1/some-endpoint -m POST -i request.json
```

### Global Options

```bash
--json          # Output in JSON format
--quiet         # Suppress non-essential output
--verbose       # Increase verbosity (-v, -vv, -vvv)
--timeout       # API request timeout in seconds (default: 30)
--insecure      # Accept invalid SSL certificates
```

## Releasing

Releases are automated via GitHub Actions. To create a new release:

1. **Update the version** in `Cargo.toml`:
   ```toml
   [package]
   version = "0.2.0"
   ```

2. **Commit the version bump**:
   ```bash
   git add Cargo.toml
   git commit -m "Bump version to 0.2.0"
   git push
   ```

3. **Create and push a version tag**:
   ```bash
   git tag v0.2.0
   git push origin v0.2.0
   ```

4. The GitHub Actions workflow will automatically:
   - Run all checks (formatting, linting, tests)
   - Build binaries for all supported platforms:
     - Linux x64 (`x86_64-unknown-linux-gnu`)
     - Linux ARM64 (`aarch64-unknown-linux-gnu`)
     - macOS Intel (`x86_64-apple-darwin`)
     - macOS Apple Silicon (`aarch64-apple-darwin`)
     - Windows x64 (`x86_64-pc-windows-msvc`)
   - Generate SHA256 checksums
   - Create a GitHub Release with all artifacts

### Pre-releases

Tags containing a hyphen (e.g., `v0.2.0-beta.1`, `v0.2.0-rc.1`) are automatically marked as pre-releases.

### Verifying Downloads

Each release includes SHA256 checksums. To verify a download:

```bash
# Download the binary and checksum
curl -LO https://github.com/dwalleck/ranch-hand/releases/download/v0.2.0/rh-v0.2.0-x86_64-unknown-linux-gnu.tar.gz
curl -LO https://github.com/dwalleck/ranch-hand/releases/download/v0.2.0/SHA256SUMS.txt

# Verify
sha256sum -c SHA256SUMS.txt --ignore-missing
```

## License

MIT
