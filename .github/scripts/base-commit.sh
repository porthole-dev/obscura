#!/bin/sh
# base-commit.sh -- print the commit this workflow run's changes are measured
# from, or nothing when there is none (scheduled and manual runs).
#
#   pull_request  the base branch commit
#   push          the previous tip of the branch; after a force push or on a
#                 new branch, the parent of the first commit the push brought
#
# Needs jq, the GitHub event payload and a checkout deep enough to hold the
# base (fetch-depth: 100, like pmaports' own CI).
set -eu
event=$GITHUB_EVENT_PATH
case "$GITHUB_EVENT_NAME" in
pull_request)
	jq -r .pull_request.base.sha "$event" ;;
push)
	before=$(jq -r .before "$event")
	if git merge-base --is-ancestor "$before" HEAD 2>/dev/null; then
		echo "$before"
	else
		first=$(jq -r '.commits[0].id // empty' "$event")
		# A root commit (the first push to a new repository) has no parent:
		# print nothing, so there is no range to check.
		if [ -n "$first" ]; then git rev-parse -q --verify "$first^" || true; fi
	fi ;;
esac
