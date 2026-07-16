#!/bin/bash

set -euo pipefail

# Parse arguments
BUILD_TYPE="debug"
if [[ "$#" -gt 0 && "$1" == "--release" ]]; then
    BUILD_TYPE="release"
fi

# Clean up old dev-symlinks.sh PATH export if present
_detect_shell_profile() {
    if [[ "${SHELL:-}" == */zsh ]]; then
        if [[ -f "$HOME/.zshrc" ]]; then
            echo "$HOME/.zshrc"
        else
            echo "$HOME/.zprofile"
        fi
    elif [[ "${SHELL:-}" == */bash ]]; then
        if [[ "$(uname)" == "Darwin" ]]; then
            if [[ -f "$HOME/.bash_profile" ]]; then
                echo "$HOME/.bash_profile"
            else
                echo "$HOME/.bashrc"
            fi
        else
            if [[ -f "$HOME/.bashrc" ]]; then
                echo "$HOME/.bashrc"
            else
                echo "$HOME/.bash_profile"
            fi
        fi
    else
        echo "$HOME/.profile"
    fi
}

_PROFILE="$(_detect_shell_profile)"
if [[ -f "$_PROFILE" ]] && grep -q '\.easylife-ai-local-dev/gitwrap/bin' "$_PROFILE"; then
    sed -i.bak '/# git-ai local dev/d' "$_PROFILE"
    sed -i.bak '/\.easylife-ai-local-dev\/gitwrap\/bin/d' "$_PROFILE"
    rm -f "$_PROFILE.bak"
    echo "Cleaned up old git-ai local dev PATH export from $_PROFILE"
fi

# Run production installer if ~/.easylife-ai isn't set up or ~/.easylife-ai/bin isn't on PATH in the profile
if [[ ! -d "$HOME/.easylife-ai/bin" ]] || [[ ! -f "$HOME/.easylife-ai/config.json" ]] || \
   { [[ -f "$_PROFILE" ]] && ! grep -q '\.easylife-ai/bin' "$_PROFILE"; } || \
   { [[ ! -f "$_PROFILE" ]]; }; then
    echo "Running git-ai installer..."
    curl -sSL https://usegitai.com/install.sh | bash
fi

# Build the binary
echo "Building $BUILD_TYPE binary..."
if [[ "$BUILD_TYPE" == "release" ]]; then
    cargo build --release
else
    cargo build
fi

# Install binary via temp file + atomic mv to avoid macOS code signature cache
# issues: direct cp reuses the inode, causing syspolicyd to fail validating the
# changed binary, leaving the process stuck in launched-suspended state unkillably.
echo "Installing binary to ~/.easylife-ai/bin/git-ai..."
TMP_BIN="$HOME/.easylife-ai/bin/git-ai.tmp.$$"
cp "target/$BUILD_TYPE/git-ai" "$TMP_BIN"
mv -f "$TMP_BIN" "$HOME/.easylife-ai/bin/git-ai"
chmod +x "$HOME/.easylife-ai/bin/git-ai"

# Run install hooks
echo "Running install hooks..."
~/.easylife-ai/bin/git-ai install

echo "Done!"
