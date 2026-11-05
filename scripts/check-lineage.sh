#!/usr/bin/env bash
# check-lineage.sh: report lineage text that the allow list does not cover.
#
# Searches every tracked file and every untracked file that .gitignore does
# not exclude (working-tree contents) for the plan's A29
# patterns (docs/craftsman-harness-plan.md, Appendix A):
#   - deepagents, deep agents, langchain, langgraph   (any case)
#   - Python                                          (capital P only)
#   - parity, mirror, byte-matched, upstream, port of (any case)
# and prints each hit as `path:line:text` unless an entry of the allow list
# covers it.
#
# Usage: scripts/check-lineage.sh [--allow FILE]
#   Runs from any directory inside the repository. FILE defaults to
#   scripts/lineage-allow.txt.
#
# Exit status: 0 when nothing was printed; 1 when it printed at least one hit;
# 2 when the allow list is malformed or has an entry that covers no hit.
#
# Allow-list format (one entry per line; blank lines and lines starting with
# `#` are ignored):
#
#   <path glob>[:<fixed substring>]  # <reason>
#
#   - The entry and its reason are separated by the FIRST occurrence of two
#     spaces, `#` and a space ("  # "). The reason must not be empty: an entry
#     without one is rejected.
#   - <path glob> is matched against the repository-relative path with bash
#     pattern rules (`*` also matches `/`, so `docs/spikes/*` covers the whole
#     directory). It must not be empty and must not contain `:`.
#   - <fixed substring>, everything after the first `:`, is optional. When it
#     is given, the entry covers only hit lines that contain it literally
#     (case-sensitive, no pattern characters). Without it the entry covers
#     every hit in the matching files; keep that for records of the past.
#   - Every entry must cover at least one hit, so stale entries are reported.
set -euo pipefail

script_dir=$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)
root=$(git -C "$script_dir" rev-parse --show-toplevel)
allow_file="$root/scripts/lineage-allow.txt"

while [ $# -gt 0 ]; do
    case "$1" in
        --allow)
            [ $# -ge 2 ] || { echo "check-lineage: --allow needs a file" >&2; exit 2; }
            case "$2" in
                /*) allow_file="$2" ;;
                *) allow_file="$PWD/$2" ;;
            esac
            shift 2
            ;;
        -h|--help)
            sed -n '2,36p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//'
            exit 0
            ;;
        *)
            echo "check-lineage: unknown argument: $1" >&2
            exit 2
            ;;
    esac
done

[ -f "$allow_file" ] || { echo "check-lineage: allow list not found: $allow_file" >&2; exit 2; }

# ---- parse the allow list ---------------------------------------------------
globs=()
subs=()
lines=()
bad=0
lineno=0
while IFS= read -r raw || [ -n "$raw" ]; do
    lineno=$((lineno + 1))
    raw=${raw%$'\r'}
    trimmed="${raw#"${raw%%[![:space:]]*}"}"
    [ -z "$trimmed" ] && continue
    case "$trimmed" in \#*) continue ;; esac
    sep='  # '
    if [[ "$trimmed" != *"$sep"* ]]; then
        echo "check-lineage: $allow_file:$lineno: entry has no reason (expected '<path>[:<substring>]  # <reason>'): $trimmed" >&2
        bad=1
        continue
    fi
    entry="${trimmed%%"$sep"*}"
    reason="${trimmed#*"$sep"}"
    reason="${reason#"${reason%%[![:space:]]*}"}"
    entry="${entry%"${entry##*[![:space:]]}"}"
    if [ -z "$reason" ]; then
        echo "check-lineage: $allow_file:$lineno: entry has an empty reason: $trimmed" >&2
        bad=1
        continue
    fi
    if [[ "$entry" == *:* ]]; then
        glob="${entry%%:*}"
        sub="${entry#*:}"
        if [ -z "$sub" ]; then
            echo "check-lineage: $allow_file:$lineno: empty substring after ':': $trimmed" >&2
            bad=1
            continue
        fi
    else
        glob="$entry"
        sub=""
    fi
    if [ -z "$glob" ]; then
        echo "check-lineage: $allow_file:$lineno: entry names no path: $trimmed" >&2
        bad=1
        continue
    fi
    globs+=("$glob")
    subs+=("$sub")
    lines+=("$lineno")
done < "$allow_file"
[ "$bad" -eq 0 ] || exit 2

# ---- search -----------------------------------------------------------------
cd "$root"
hits=$(
    {
        git grep --untracked -n -I -i -E -e 'deepagents|deep agents|langchain|langgraph' || true
        git grep --untracked -n -I -E -e 'Python' || true
        git grep --untracked -n -I -i -E -e 'parity|mirror|byte-matched|upstream|port of' || true
    } | LC_ALL=C sort -t: -k1,1 -k2,2n -u
)

# ---- filter -----------------------------------------------------------------
used=()
for i in "${!globs[@]}"; do used[i]=0; done
printed=0
if [ -n "$hits" ]; then
    while IFS= read -r hit; do
        path="${hit%%:*}"
        rest="${hit#*:}"
        text="${rest#*:}"
        covered=0
        for i in "${!globs[@]}"; do
            # shellcheck disable=SC2053 # the glob is a pattern on purpose
            if [[ "$path" == ${globs[i]} ]]; then
                if [ -z "${subs[i]}" ] || [[ "$text" == *"${subs[i]}"* ]]; then
                    covered=1
                    used[i]=1
                fi
            fi
        done
        if [ "$covered" -eq 0 ]; then
            printf '%s\n' "$hit"
            printed=1
        fi
    done <<< "$hits"
fi

stale=0
for i in "${!globs[@]}"; do
    if [ "${used[i]}" -eq 0 ]; then
        echo "check-lineage: $allow_file:${lines[i]}: entry covers no hit (remove it): ${globs[i]}${subs[i]:+:${subs[i]}}" >&2
        stale=1
    fi
done

[ "$printed" -eq 0 ] || exit 1
[ "$stale" -eq 0 ] || exit 2
exit 0
