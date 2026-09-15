#!/bin/sh
# test-commit-check.sh -- run commit-check.sh against throwaway commits.
set -eu
check=$(cd "$(dirname "$0")" && pwd)/commit-check.sh
repo=$(mktemp -d)
trap 'rm -rf "$repo"' EXIT
cd "$repo"
git init -q
git config user.name "Jane Doe"
git config user.email jane@example.org
git commit -q --allow-empty -m base

# expect pass|fail, extra check flags, commit message
expect() {
	result=$1 flags=$2
	git commit -q --allow-empty -m "$3"
	if sh "$check" HEAD~1 HEAD $flags >/dev/null 2>&1; then got=pass; else got=fail; fi
	if [ "$got" != "$result" ]; then
		echo "FAIL: expected $result, got $got for: $3" >&2
		exit 1
	fi
}

sob="Signed-off-by: Jane Doe <jane@example.org>"
expect pass "" "human change, no trailers"
expect pass "" "change

Assisted-by: Claude"
expect pass "" "change

Generated-by: some-tool 1.0"
expect pass --dco "change

Assisted-by: Claude
$sob"
expect pass --dco "human change

$sob"
expect fail --dco "change without sign-off"
expect fail --dco "change

Signed-off-by: Someone Else <else@example.org>"
expect fail "" "change

Co-authored-by: Claude <noreply@anthropic.com>"
expect fail "" "change

Co-developed-by: GitHub Copilot"
expect fail "" "change

Signed-off-by: Claude <noreply@anthropic.com>"
expect fail "" "change

Signed-off-by: renovate[bot] <bot@example.org>"
expect fail "" "change

Claude-Session: 0123"
expect fail "" "change

🤖 Generated with [Claude Code](https://claude.com/claude-code)"
expect fail "" "change

Assisted-by:"
expect fail "" "change

assisted-by: claude"
echo "commit-check: all cases pass"
