#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$repo_root"
cargo build --package rt --package rs --release

mkdir -p "$HOME/bin"
cp "$repo_root/target/release/rt" "$HOME/bin/rt"
chmod +x "$HOME/bin/rt"
cp "$repo_root/target/release/rs" "$HOME/bin/rs"
chmod +x "$HOME/bin/rs"

echo "Deployed rt and rs to $HOME/bin"
