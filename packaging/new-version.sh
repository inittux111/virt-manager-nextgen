#!/bin/sh
# Moves the app to its next version and records it in git:
#
#   packaging/new-version.sh [patch|minor|major|X.Y.Z]   (default: patch)
#
# Updates Cargo.toml, Cargo.lock, the spec (Version + changelog) and the
# AppStream release list. The "what's new" notes are the commit titles since
# the previous version's tag. Then commits those files and tags vX.Y.Z.
# build-rpm.sh calls this before every build.
set -eu

cd "$(dirname "$0")/.."
spec=packaging/virt-manager-nextgen.spec
metainfo=data/io.github.inittux111.VirtManagerNextgen.metainfo.xml

current=$(sed -n 's/^Version: *//p' "$spec")
IFS=. read -r major minor patch <<EOF
$current
EOF
case "${1:-patch}" in
    patch) next="$major.$minor.$((patch + 1))" ;;
    minor) next="$major.$((minor + 1)).0" ;;
    major) next="$((major + 1)).0.0" ;;
    [0-9]*.[0-9]*.[0-9]*) next=$1 ;;
    *) echo "Usage: $0 [patch|minor|major|X.Y.Z]" >&2; exit 2 ;;
esac
if git rev-parse -q --verify "refs/tags/v$next" >/dev/null; then
    echo "Version $next exists already (tag v$next)." >&2
    exit 1
fi

# What's new: commit titles since the last version tag.
since=$(git describe --tags --abbrev=0 --match 'v[0-9]*' 2>/dev/null || true)
notes=$(git log --no-merges --format=%s ${since:+"$since"..HEAD} | grep -v '^Release ' || true)
[ -n "$notes" ] || notes="Small fixes and improvements"

echo "Version $current → $next"

# Cargo.toml (the workspace version line) and the lock file.
sed -i "s/^version = \"$current\"/version = \"$next\"/" Cargo.toml
toolbox run -c virt-manager-nextgen env CARGO_HOME=/opt/cargo cargo update --workspace --offline --quiet

# The spec: version, and a changelog entry on top.
who="$(git config user.name) <$(git config user.email)>"
entry=$(mktemp)
{
    echo "* $(LC_ALL=C date '+%a %b %d %Y') $who - $next-1"
    # rpm expands % in the changelog, so double it.
    echo "$notes" | sed -e 's/%/%%/g' -e 's/^/- /'
    echo
} >"$entry"
sed -i "s/^Version: .*/Version:        $next/" "$spec"
sed -i "/^%changelog$/r $entry" "$spec"
rm -f "$entry"

# AppStream: a <release> on top of the list (text escaped for XML).
release=$(mktemp)
{
    echo "    <release version=\"$next\" date=\"$(date +%F)\">"
    echo "      <description>"
    echo "        <ul>"
    echo "$notes" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g' -e 's|.*|          <li>&</li>|'
    echo "        </ul>"
    echo "      </description>"
    echo "    </release>"
} >"$release"
sed -i "/<releases>/r $release" "$metainfo"
rm -f "$release"

git commit -q -m "Release $next" -- Cargo.toml Cargo.lock "$spec" "$metainfo"
git tag -a "v$next" -m "Version $next"
echo "Committed and tagged v$next."
