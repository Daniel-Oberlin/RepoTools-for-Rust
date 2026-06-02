#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"

cd "$repo_root"
cargo build --package rt --release

mkdir -p "$HOME/bin"
cp "$repo_root/target/release/rt" "$HOME/bin/rt"
chmod +x "$HOME/bin/rt"

echo "Deployed rt to $HOME/bin/rt"
