#!/usr/bin/env bash
# Claude Code hook: CI-equivalent Rust check at the end of a turn, only when Rust changed.
#
#   rust-check.sh mark   PostToolUse(Edit|Write): record touched Rust/Cargo/.cu files (silent)
#   rust-check.sh stop   Stop: rustfmt touched files (silent), then cargo clippy; report
#                        errors anywhere + warnings in touched files as Stop hook feedback.
#                        Touched CUDA kernels (.cu) are also compiled with nvcc, when present:
#                        NVRTC compiles them only at runtime, so nothing else catches an error.
#
# State: ${TMPDIR:-/tmp}/claude-rust-check/<session_id>/{files,blocks}
# No touched files -> the Stop hook exits immediately without starting cargo.
set -uo pipefail

MAX_BLOCKS=3
MAX_LINES=40
CLIPPY_TIMEOUT=540
NVCC_TIMEOUT=60

mode="${1:-}"
input="$(cat)"
session="$(jq -r '.session_id // "default"' <<<"$input" 2>/dev/null)"
[ -n "$session" ] || session=default
state="${TMPDIR:-/tmp}/claude-rust-check/${session//[^A-Za-z0-9_-]/_}"

emit_system_message() {
    jq -n --arg m "rust-check: $1" '{systemMessage: $m}'
}

case "$mode" in
mark)
    file="$(jq -r '.tool_input.file_path // empty' <<<"$input" 2>/dev/null)"
    case "$file" in
    *.rs | *.cu | */Cargo.toml | */Cargo.lock | */clippy.toml | */rustfmt.toml | */.cargo/config.toml) ;;
    *) exit 0 ;;
    esac
    mkdir -p "$state" || exit 0
    grep -qxF -- "$file" "$state/files" 2>/dev/null || printf '%s\n' "$file" >>"$state/files"
    echo 0 >"$state/blocks"
    exit 0
    ;;
stop) ;;
*) exit 0 ;;
esac

[ -s "$state/files" ] || exit 0
blocks="$(cat "$state/blocks" 2>/dev/null || echo 0)"
# Past the cap: stay quiet until the next Rust edit resets the counter.
[ "$blocks" -gt "$MAX_BLOCKS" ] 2>/dev/null && exit 0

mapfile -t touched <"$state/files"

# Group touched files by repository root (works in worktrees too).
declare -A roots=()
for f in "${touched[@]}"; do
    dir="$(dirname -- "$f")"
    [ -d "$dir" ] || continue
    root="$(git -C "$dir" rev-parse --show-toplevel 2>/dev/null)" || continue
    [ -f "$root/Cargo.toml" ] && roots["$root"]=1
done
if [ "${#roots[@]}" -eq 0 ]; then
    rm -rf -- "$state"
    exit 0
fi

report=()
extra_warnings=0
infra_error=""

# nvcc is not on PATH by default; without a CUDA toolkit the kernel check is skipped.
nvcc="$(command -v nvcc 2>/dev/null)"
[ -n "$nvcc" ] || { [ -x /usr/local/cuda/bin/nvcc ] && nvcc=/usr/local/cuda/bin/nvcc; }

for root in "${!roots[@]}"; do
    # Repo-relative touched paths for this root.
    rel=()
    for f in "${touched[@]}"; do
        case "$f" in "$root"/*) rel+=("${f#"$root"/}") ;; esac
    done

    # 1. Format touched .rs files without reporting anything (rustfmt.toml is picked up).
    fmt=()
    for r in "${rel[@]}"; do
        [[ "$r" == *.rs && -f "$root/$r" ]] && fmt+=("$root/$r")
    done
    [ "${#fmt[@]}" -gt 0 ] && rustfmt --quiet -- "${fmt[@]}" >/dev/null 2>&1

    # 2. Clippy: the workspace, plus any excluded tools/<crate> that was touched.
    #    Each entry is "<path prefix to prepend>|<manifest path or empty>".
    runs=("|")
    declare -A tool_seen=()
    for r in "${rel[@]}"; do
        if [[ "$r" =~ ^(tools/[^/]+)/ ]] && [ -f "$root/${BASH_REMATCH[1]}/Cargo.toml" ]; then
            t="${BASH_REMATCH[1]}"
            [ -n "${tool_seen[$t]:-}" ] || { tool_seen[$t]=1; runs+=("$t/|$t/Cargo.toml"); }
        fi
    done
    unset tool_seen

    for run in "${runs[@]}"; do
        prefix="${run%%|*}"
        manifest="${run#*|}"
        args=(clippy --all-targets --message-format=short --color=never)
        if [ -n "$manifest" ]; then args+=(--manifest-path "$manifest"); else args+=(--workspace); fi

        out="$(cd "$root" && timeout "$CLIPPY_TIMEOUT" cargo "${args[@]}" 2>&1)"
        status=$?
        [ "$status" -eq 124 ] && { infra_error="cargo clippy timed out after ${CLIPPY_TIMEOUT}s in $root"; continue; }

        diag_count=0
        while IFS= read -r line; do
            [[ "$line" =~ ^([^:[:space:]][^:]*):[0-9]+:[0-9]+:\ (error|warning)(\[[^]]*\])?:\  ]] || continue
            diag_count=$((diag_count + 1))
            path="${prefix}${BASH_REMATCH[1]}"
            kind="${BASH_REMATCH[2]}"
            line="${prefix}${line}"
            if [ "$kind" = error ]; then
                report+=("$line")
            else
                hit=0
                for r in "${rel[@]}"; do [ "$r" = "$path" ] && { hit=1; break; }; done
                if [ "$hit" -eq 1 ]; then report+=("$line"); else extra_warnings=$((extra_warnings + 1)); fi
            fi
        done <<<"$out"

        if [ "$status" -ne 0 ] && [ "$diag_count" -eq 0 ]; then
            infra_error="cargo clippy failed in $root: $(grep -m1 -E '^error' <<<"$out" || tail -n1 <<<"$out")"
        fi
    done

    # 3. CUDA kernels: compile each touched .cu file to PTX and discard the output.
    [ -n "$nvcc" ] || continue
    for r in "${rel[@]}"; do
        [[ "$r" == *.cu && -f "$root/$r" ]] || continue
        out="$(cd "$root" && timeout "$NVCC_TIMEOUT" "$nvcc" -ptx -o /dev/null "$r" 2>&1)"
        status=$?
        [ "$status" -eq 124 ] && { infra_error="nvcc timed out after ${NVCC_TIMEOUT}s on $r"; continue; }
        diag_count=0
        while IFS= read -r line; do
            [[ "$line" =~ ^[^[:space:]].*\([0-9]+\):\ (catastrophic\ )?(error|warning) ]] || continue
            diag_count=$((diag_count + 1))
            report+=("$line")
        done <<<"$out"
        if [ "$status" -ne 0 ] && [ "$diag_count" -eq 0 ]; then
            infra_error="nvcc failed on $r: $(grep -m1 . <<<"$out")"
        fi
    done
done

if [ "${#report[@]}" -eq 0 ]; then
    if [ -n "$infra_error" ]; then
        # Environment problem, not a code problem: tell the user, do not make Claude loop on it.
        emit_system_message "$infra_error"
        rm -rf -- "$state"
        exit 0
    fi
    # Clean (warnings elsewhere are pre-existing and do not block).
    rm -rf -- "$state"
    exit 0
fi

total="${#report[@]}"
body="$(printf '%s\n' "${report[@]:0:$MAX_LINES}")"
[ "$total" -gt "$MAX_LINES" ] && body+=$'\n'"... $((total - MAX_LINES)) more"
[ "$extra_warnings" -gt 0 ] && body+=$'\n'"$extra_warnings more warning(s) in untouched files (CI runs clippy with -D warnings)"

echo "$((blocks + 1))" >"$state/blocks"
if [ "$blocks" -ge "$MAX_BLOCKS" ]; then
    # Claude already got MAX_BLOCKS rounds of feedback: tell the user once, do not continue the turn.
    emit_system_message "the check still fails after $MAX_BLOCKS fix attempts; quiet until the next Rust or .cu edit."$'\n'"$body"
else
    msg="cargo clippy (CI gate, --workspace --all-targets) and nvcc (touched .cu files) report problems in this turn's changes. Fix them before finishing:"$'\n'"$body"
    jq -n --arg m "$msg" '{hookSpecificOutput: {hookEventName: "Stop", additionalContext: $m}}'
fi
exit 0
