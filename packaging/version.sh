#!/bin/sh
# Prints the app's current version: the workspace version in Cargo.toml, the
# one place both build scripts (RPM and Debian) read it from.
cd "$(dirname "$0")/.."
sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"/\1/p' Cargo.toml
