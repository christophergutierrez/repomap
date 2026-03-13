#!/bin/sh
set -e

REPO="christophergutierrez/repomap"
BINARY="repomap"
INSTALL_DIR="$HOME/.local/bin"

main() {
    os="$(uname -s)"
    arch="$(uname -m)"

    case "$os" in
        Linux)  os_target="unknown-linux-gnu" ;;
        Darwin) os_target="apple-darwin" ;;
        *)
            echo "Error: unsupported OS: $os" >&2
            echo "Try: cargo install --git https://github.com/$REPO" >&2
            exit 1
            ;;
    esac

    case "$arch" in
        x86_64|amd64)   arch_target="x86_64" ;;
        aarch64|arm64)   arch_target="aarch64" ;;
        *)
            echo "Error: unsupported architecture: $arch" >&2
            echo "Try: cargo install --git https://github.com/$REPO" >&2
            exit 1
            ;;
    esac

    target="${arch_target}-${os_target}"
    archive="${BINARY}-${target}.tar.gz"
    url="https://github.com/${REPO}/releases/latest/download/${archive}"

    echo "Detected platform: ${target}"
    echo "Downloading ${BINARY}..."

    tmpdir="$(mktemp -d)"
    trap 'rm -rf "$tmpdir"' EXIT

    if ! curl -fsSL "$url" -o "${tmpdir}/${archive}"; then
        echo "Error: failed to download ${url}" >&2
        echo "No release found for your platform. Try building from source:" >&2
        echo "  cargo install --git https://github.com/$REPO" >&2
        exit 1
    fi

    tar xzf "${tmpdir}/${archive}" -C "$tmpdir"

    mkdir -p "$INSTALL_DIR"
    mv "${tmpdir}/${BINARY}" "${INSTALL_DIR}/${BINARY}"
    chmod +x "${INSTALL_DIR}/${BINARY}"

    if "${INSTALL_DIR}/${BINARY}" --version > /dev/null 2>&1; then
        echo "Installed ${BINARY} to ${INSTALL_DIR}/${BINARY}"
    else
        echo "Warning: binary installed but --version check failed" >&2
    fi

    case ":$PATH:" in
        *":${INSTALL_DIR}:"*) ;;
        *)
            echo ""
            echo "Add ${INSTALL_DIR} to your PATH:"
            echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
            ;;
    esac
}

main
