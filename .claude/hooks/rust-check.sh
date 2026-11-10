#!/usr/bin/env bash
# Claude Code hook: CI-equivalent Rust check at the end of a turn, only when Rust changed.
#
#   rust-check.sh mark   PostToolUse(Edit|Write): record touched Rust/Cargo/.cu/.ptx files (silent)
#   rust-check.sh stop   Stop: rustfmt touched files (silent), then cargo clippy; report
#                        errors anywhere + warnings in touched files as Stop hook feedback.
#                        Touched CUDA kernels (.cu) are also compiled with nvcc, when present,
#                        and a touched kernels.cu or kernels.ptx runs scripts/gen-kernels-ptx.sh
#                        --check (needs NVRTC 12.6), which reports a stale embedded PTX.
#                        The file-size note comes from the global ship hook (size-check.sh), which
#                        reads .claude/workflow.md "## Size limits"; this hook does not give it.
#
# State: ${TMPDIR:-/tmp}/claude-rust-check/<session_id>/{files,blocks}
# No touched files -> the Stop hook exits immediately without starting cargo.
#
# Builder hold: while a builder agent owns a checkout, `stop` does not format or lint it.
# /ship:roadmap-next writes <git dir>/claude-builder.<run>.hold before it starts a builder
# (opus-professor, fable-builder) and deletes that one file when the builder returns. The main
# session ends its turn while it waits, and without the hold this hook would rustfmt files the
# builder is still editing and take the cargo lock. The git dir is the checkout's own (a linked
# worktree has its own), and every run has its own file, so two runs never clear each other's
# hold and a hold never reaches another checkout. `mark` renews the holds of the checkout an edit
# lands in; a hold older than HOLD_MAX_MIN minutes (a dead session) no longer counts. The touched
# files of a held checkout stay recorded, so the first Stop after the builder returns checks them.
set -uo pipefail

MAX_BLOCKS=3
MAX_LINES=40
CLIPPY_TIMEOUT=540
NVCC_TIMEOUT=60
HOLD_MAX_MIN=180

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
    # An edit in a checkout with builder holds keeps them fresh, whatever the file type.
    if [ -n "$file" ]; then
        gd="$(git -C "$(dirname -- "$file")" rev-parse --absolute-git-dir 2>/dev/null)"
        if [ -n "$gd" ]; then
            for h in "$gd"/claude-builder.*.hold; do [ -e "$h" ] && touch -c -- "$h"; done
        fi
    fi
    case "$file" in
    *.rs | *.cu | *.ptx | */Cargo.toml | */Cargo.lock | */clippy.toml | */rustfmt.toml | */.cargo/config.toml) ;;
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

# Leave out every checkout that a builder holds, and keep its touched files for a later Stop.
held=()
for root in "${!roots[@]}"; do
    gd="$(git -C "$root" rev-parse --absolute-git-dir 2>/dev/null)" || continue
    if [ -n "$(find "$gd" -maxdepth 1 -name 'claude-builder.*.hold' -mmin "-$HOLD_MAX_MIN" -print -quit 2>/dev/null)" ]; then
        held+=("$root")
    fi
done
keep=()
for root in "${held[@]}"; do
    unset 'roots[$root]'
    for f in "${touched[@]}"; do
        case "$f" in "$root"/*) keep+=("$f") ;; esac
    done
done
# Done with this turn's files: forget them, except the ones a held checkout still owes a check.
finish_state() {
    if [ "${#keep[@]}" -gt 0 ]; then
        printf '%s\n' "${keep[@]}" >"$state/files"
    else
        rm -rf -- "$state"
    fi
}
if [ "${#roots[@]}" -eq 0 ]; then
    finish_state
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

    # 3. The embedded PTX must be NVRTC 12.6's output for kernels.cu. Only report: the hook's
    #    nvcc is another compiler, so regenerating here would write the wrong file.
    ptx_touched=0
    for r in "${rel[@]}"; do
        case "$r" in */kernels.cu | */kernels.ptx) ptx_touched=1 ;; esac
    done
    if [ "$ptx_touched" -eq 1 ] && [ -x "$root/scripts/gen-kernels-ptx.sh" ]; then
        out="$(cd "$root" && timeout "$NVCC_TIMEOUT" scripts/gen-kernels-ptx.sh --check 2>&1)"
        case $? in
        0) ;;
        1)
            # The stale line, not NVRTC's compiler log that may come first; a compile error
            # has no ::error:: prefix.
            line="$(grep -m1 '^::error::' <<<"$out" || grep -m1 . <<<"$out")"
            report+=("${line#::error::}")
            ;;
        *) infra_error="kernel PTX check did not run: $(grep -m1 . <<<"$out")" ;;
        esac
    fi

    # 4. CUDA kernels: compile each touched .cu file to PTX and discard the output.
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
        finish_state
        exit 0
    fi
    # Clean (warnings elsewhere are pre-existing and do not block).
    finish_state
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
    msg="cargo clippy (CI gate, --workspace --all-targets), the kernel PTX check and nvcc (touched .cu files) report problems in this turn's changes. Fix them before finishing:"$'\n'"$body"
    jq -n --arg m "$msg" '{hookSpecificOutput: {hookEventName: "Stop", additionalContext: $m}}'
fi
exit 0
