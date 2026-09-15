#!/bin/sh
# commit-check.sh BASE HEAD [--dco] -- check the trailers of the commits in
# BASE..HEAD.
#
# Rejected in every commit, whoever wrote it:
#   - an AI tool named in Co-authored-by: or Co-developed-by:
#   - an AI tool or bot named in Signed-off-by: (a sign-off is a person's
#     Developer Certificate of Origin)
#   - Claude-Session: trailers, AI session URLs, "Generated with [...]" lines
#   - a malformed Assisted-by: or Generated-by: trailer. Both are optional;
#     they are expected when an AI tool was involved (see CONTRIBUTING.md).
# With --dco (pull requests), every commit also needs a Signed-off-by:
# matching its author.
set -eu
base=$1 head=$2 dco=${3:-}
ai='claude|anthropic|openai|chatgpt|copilot|gemini|codex|cursor agent|\[bot\]'
wrong="^(co-authored-by|co-developed-by):.*($ai)|^signed-off-by:.*($ai)|^claude-session:|claude\.ai/code/session_|chatgpt\.com/(share|c)/|^.{0,8}generated with \["
fail=0
n=0
for c in $(git rev-list "$base..$head"); do
	n=$((n + 1))
	short=$(git rev-parse --short "$c")
	msg=$(git log -1 --format=%B "$c")
	if printf '%s\n' "$msg" | grep -qiE "$wrong"; then
		echo "::error::$short: AI attribution in a trailer or line where it does not belong"
		fail=1
	fi
	if printf '%s\n' "$msg" | grep -iE '^(assisted|generated)-by:' | grep -qvE '^(Assisted|Generated)-by: [^[:space:]]'; then
		echo "::error::$short: malformed Assisted-by: or Generated-by: trailer (expected 'Assisted-by: <tool>')"
		fail=1
	fi
	if [ "$dco" = --dco ]; then
		author=$(git log -1 --format='%an <%ae>' "$c")
		if ! git log -1 --format='%(trailers:key=Signed-off-by,valueonly)' "$c" | grep -qxF "$author"; then
			echo "::error::$short: no Signed-off-by: matching the commit author (git commit -s, or git rebase --signoff)"
			fail=1
		fi
	fi
done
echo "checked $n commit(s)"
exit "$fail"
