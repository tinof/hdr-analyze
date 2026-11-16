#!/usr/bin/env bash
# Tests for the builder hold of ../rust-check.sh, in a throwaway repository with a linked worktree
# and stub `cargo` and `rustfmt` on PATH (no real build runs). While a fresh
# <git dir>/claude-builder.<run>.hold exists, `stop` neither formats nor lints that checkout and
# keeps its touched files; a stale hold does not count; two runs do not clear each other's hold;
# a hold in one checkout does not hold another, also for a worktree nested inside the main
# checkout; `mark` renews the holds of the checkout it edits. It also checks that the hook keeps to
# bash 3.2 syntax (macOS /bin/bash), that clippy diagnostics are reported for touched files only,
# that a touched tools/<crate> gets one clippy run of its own, and the timeout fallbacks.
#
# Usage: .claude/hooks/tests/rust-check-hold-test.sh
#        RUST_CHECK_TEST_BASH=/bin/bash .claude/hooks/tests/rust-check-hold-test.sh   (macOS 3.2)
set -u
here=$(cd "$(dirname "$0")" && pwd)
hook="$here/../rust-check.sh"
# The shell that runs the hook (the test itself runs under any bash).
hook_bin=$(command -v "${RUST_CHECK_TEST_BASH:-bash}") || { echo "FAIL no hook shell: ${RUST_CHECK_TEST_BASH:-bash}"; exit 1; }
hook_ver=$("$hook_bin" -c 'echo $BASH_VERSION')
echo "hook shell: $hook_bin (bash $hook_ver)"
work=$(mktemp -d "${TMPDIR:-/tmp}/rust-check-hold-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
check() { local name=$1; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }

# 0. No bash 4 syntax: macOS /bin/bash 3.2 has no mapfile/readarray, no associative arrays and no
#    ${!name} expansions. Comment lines are left out (the header names what it avoids).
bash4=$(grep -v '^[[:space:]]*#' "$hook" | grep -nE '(^|[^[:alnum:]_])(mapfile|readarray)([^[:alnum:]_]|$)|(declare|local|typeset) +-[[:alpha:]]*A|\$\{!')
if [ -z "$bash4" ]; then ok "the hook uses no bash 4 syntax"; else bad "the hook uses no bash 4 syntax:"$'\n'"$bash4"; fi

# Stubs: record every call with the directory it ran in (rustfmt also with its arguments, so a
# test sees which files it formatted), succeed. cargo also records its arguments in cargo-args and
# prints cargo-out when that file exists, as clippy's diagnostics.
mkdir -p "$work/bin" "$work/tmp"
printf '#!/usr/bin/env bash\necho "cargo $PWD" >> "%s/calls"\necho "$*" >> "%s/cargo-args"\n[ -f "%s/cargo-out" ] && cat "%s/cargo-out"\nexit 0\n' \
    "$work" "$work" "$work" "$work" > "$work/bin/cargo"
printf '#!/usr/bin/env bash\necho "rustfmt $PWD $*" >> "%s/calls"\n' "$work" > "$work/bin/rustfmt"
chmod +x "$work/bin/cargo" "$work/bin/rustfmt"
# Set a file's mtime <minutes> back. GNU date takes -d, BSD date (macOS) takes -v.
age() { touch -t "$(date -d "-$1 minutes" +%Y%m%d%H%M 2>/dev/null || date -v-"$1"M +%Y%m%d%H%M)" "$2"; }
export PATH="$work/bin:$PATH" TMPDIR="$work/tmp"

a="$work/a"
git init -q "$a"
printf '[package]\nname = "a"\nversion = "0.0.0"\n' > "$a/Cargo.toml"
mkdir -p "$a/src" && echo 'pub fn f() {}' > "$a/src/lib.rs" && echo doc > "$a/README.md"
git -C "$a" add -A
git -C "$a" -c user.name=t -c user.email=t@example.invalid commit -q -m init
b="$work/b"
git -C "$a" worktree add -q "$b" -b wt
ga=$(git -C "$a" rev-parse --absolute-git-dir)
gb=$(git -C "$b" rev-parse --absolute-git-dir)
[ "$ga" != "$gb" ] && ok "the worktree has its own git dir" || bad "the worktree has its own git dir"

session=hold-test
state="$work/tmp/claude-rust-check/$session"
mark() { printf '{"session_id":"%s","tool_input":{"file_path":"%s"}}' "$session" "$1" | "$hook_bin" "$hook" mark; }
stop() { : > "$work/calls"; : > "$work/cargo-args"; out=$(printf '{"session_id":"%s"}' "$session" | "$hook_bin" "$hook" stop 2>&1); }
cargo_ran_in() { grep -qxF "cargo $1" "$work/calls"; }
no_calls() { [ ! -s "$work/calls" ]; }
recorded() { grep -qxF -- "$1" "$state/files" 2>/dev/null; }

# 1. No hold: the check runs and the state is cleared.
mark "$a/src/lib.rs"; stop
check "no hold: clippy runs" cargo_ran_in "$a"
check "no hold: rustfmt runs" grep -q '^rustfmt ' "$work/calls"
check "no hold: state cleared" test ! -e "$state"

# 2. A fresh hold: nothing runs, nothing is printed, the file stays recorded.
mark "$a/src/lib.rs"; echo "opus-professor R1.1" > "$ga/claude-builder.r1-1-100.hold"; stop
check "fresh hold: no rustfmt, no clippy" no_calls
check "fresh hold: no output" test -z "$out"
check "fresh hold: the file stays recorded" recorded "$a/src/lib.rs"

# 3. The hold is removed: the next Stop checks the builder's file.
rm "$ga/claude-builder.r1-1-100.hold"; stop
check "hold removed: clippy runs" cargo_ran_in "$a"
check "hold removed: state cleared" test ! -e "$state"

# 4. A stale hold (a dead session) does not count. Create it after the mark: a mark renews holds.
mark "$a/src/lib.rs"; echo stale > "$ga/claude-builder.dead-1.hold"; age 240 "$ga/claude-builder.dead-1.hold"; stop
check "stale hold: clippy runs" cargo_ran_in "$a"
rm "$ga/claude-builder.dead-1.hold"

# 5. Two runs in one checkout: removing one hold leaves the other in force.
mark "$a/src/lib.rs"; echo one > "$ga/claude-builder.r1-1-200.hold"; echo two > "$ga/claude-builder.r2-1-201.hold"
rm "$ga/claude-builder.r1-1-200.hold"; stop
check "overlapping runs: the second hold still holds" no_calls
rm "$ga/claude-builder.r2-1-201.hold"; stop
check "overlapping runs: free after both are gone" cargo_ran_in "$a"

# 6. A hold belongs to its checkout: one in the worktree does not hold the main checkout, and the
#    worktree's file stays recorded while the main checkout's is checked.
mark "$a/src/lib.rs"; mark "$b/src/lib.rs"; echo wt > "$gb/claude-builder.r3-1-300.hold"; stop
check "other checkout: the main checkout is checked" cargo_ran_in "$a"
if cargo_ran_in "$b"; then bad "other checkout: the held worktree is not checked"; else ok "other checkout: the held worktree is not checked"; fi
check "other checkout: the worktree's file stays recorded" recorded "$b/src/lib.rs"
if recorded "$a/src/lib.rs"; then bad "other checkout: the checked file is forgotten"; else ok "other checkout: the checked file is forgotten"; fi
rm "$gb/claude-builder.r3-1-300.hold"; stop
check "other checkout: the worktree is checked once its hold is gone" cargo_ran_in "$b"

# 7. An edit renews the holds of its own checkout, whatever the file type, and no other's.
echo x > "$ga/claude-builder.r4-1-400.hold"; age 120 "$ga/claude-builder.r4-1-400.hold"
echo y > "$gb/claude-builder.r5-1-401.hold"; age 120 "$gb/claude-builder.r5-1-401.hold"
mark "$a/README.md"
check "renewal: the edited checkout's hold is fresh" test -n "$(find "$ga" -maxdepth 1 -name 'claude-builder.r4-1-400.hold' -mmin -5)"
check "renewal: another checkout's hold is untouched" test -z "$(find "$gb" -maxdepth 1 -name 'claude-builder.r5-1-401.hold' -mmin -5)"
check "renewal: a non-Rust edit records nothing" test ! -e "$state"
rm "$ga/claude-builder.r4-1-400.hold" "$gb/claude-builder.r5-1-401.hold"

# 8. A worktree nested inside the main checkout (the repo's .claude/worktrees/<name> layout): each
#    file belongs to its own checkout, not to every checkout whose path is a prefix of it.
c="$a/.claude/worktrees/c"
git -C "$a" worktree add -q "$c" -b nested
gc=$(git -C "$c" rev-parse --absolute-git-dir)
formatted() { grep '^rustfmt ' "$work/calls" | grep -qF -- "$1"; }
mark "$a/src/lib.rs"; mark "$c/src/lib.rs"; echo nested > "$gc/claude-builder.r6-1-500.hold"; stop
check "nested, worktree held: the main checkout is checked" cargo_ran_in "$a"
if cargo_ran_in "$c"; then bad "nested, worktree held: the worktree is not linted"; else ok "nested, worktree held: the worktree is not linted"; fi
if formatted "$c/src/lib.rs"; then bad "nested, worktree held: the worktree's file is not formatted"; else ok "nested, worktree held: the worktree's file is not formatted"; fi
check "nested, worktree held: the worktree's file stays recorded" recorded "$c/src/lib.rs"
rm "$gc/claude-builder.r6-1-500.hold"; stop
check "nested, worktree held: the worktree is checked once its hold is gone" cargo_ran_in "$c"
mark "$a/src/lib.rs"; mark "$c/src/lib.rs"; echo main > "$ga/claude-builder.r7-1-501.hold"; stop
check "nested, main held: the worktree is checked" cargo_ran_in "$c"
if cargo_ran_in "$a"; then bad "nested, main held: the main checkout is not linted"; else ok "nested, main held: the main checkout is not linted"; fi
check "nested, main held: the main checkout's file stays recorded" recorded "$a/src/lib.rs"
if recorded "$c/src/lib.rs"; then bad "nested, main held: the checked worktree file is forgotten"; else ok "nested, main held: the checked worktree file is forgotten"; fi
rm "$ga/claude-builder.r7-1-501.hold"; stop

# 9. An edit does not revive an expired hold: a hold older than the limit stays ignored even when
#    it existed before the mark, so the next Stop checks the checkout.
echo dead > "$ga/claude-builder.dead-2.hold"; age 240 "$ga/claude-builder.dead-2.hold"
mark "$a/src/lib.rs"; stop
check "expired hold before an edit: clippy runs" cargo_ran_in "$a"
check "expired hold before an edit: the hold stays expired" test -z "$(find "$ga" -maxdepth 1 -name 'claude-builder.dead-2.hold' -mmin -180)"
rm "$ga/claude-builder.dead-2.hold"

# 10. Diagnostics: errors are reported from anywhere, warnings only for touched files (the others
#     are counted), and a touched tools/<crate> gets exactly one clippy run with its manifest and
#     its paths prefixed. The cargo stub prints the same three lines for both runs.
mkdir -p "$a/tools/t/src"
printf '[package]\nname = "t"\nversion = "0.0.0"\n' > "$a/tools/t/Cargo.toml"
echo 'pub fn g() {}' > "$a/tools/t/src/lib.rs"
printf '%s\n' 'src/lib.rs:1:1: warning: touched warn' 'src/other.rs:1:1: warning: untouched warn' \
    'src/err.rs:2:3: error[E0001]: boom' 'note: not a diagnostic line' > "$work/cargo-out"
reported() { printf '%s\n' "$out" | jq -r '.hookSpecificOutput.additionalContext // empty' | grep -qxF -- "$1"; }
mark "$a/src/lib.rs"; mark "$a/tools/t/src/lib.rs"; mark "$a/tools/t/Cargo.toml"; stop
check "diagnostics: a warning in a touched file is reported" reported 'src/lib.rs:1:1: warning: touched warn'
check "diagnostics: a touched tools/ file's warning is reported with its prefix" reported 'tools/t/src/lib.rs:1:1: warning: touched warn'
check "diagnostics: an error is reported" reported 'src/err.rs:2:3: error[E0001]: boom'
check "diagnostics: a tools/ error is reported with its prefix" reported 'tools/t/src/err.rs:2:3: error[E0001]: boom'
if printf '%s\n' "$out" | grep -qF 'untouched warn'; then bad "diagnostics: an untouched file's warning is not reported"; else ok "diagnostics: an untouched file's warning is not reported"; fi
check "diagnostics: untouched warnings are counted" reported '2 more warning(s) in untouched files (CI runs clippy with -D warnings)'
check "diagnostics: one workspace run" test "$(grep -c -- '--workspace' "$work/cargo-args")" -eq 1
check "diagnostics: one tools/t run with its manifest" test "$(grep -c -- '--manifest-path tools/t/Cargo.toml' "$work/cargo-args")" -eq 1
rm -f "$work/cargo-out"; rm -rf -- "$state"

# 11. Timeout fallbacks, on a PATH that holds only the tools the hook and the stubs need: without
#     timeout(1) the hook uses gtimeout, and without either it runs clippy unbounded.
farm="$work/farm"
mkdir -p "$farm"
for t in jq git cat dirname find touch mkdir grep rm tail; do
    p=$(command -v "$t") || { bad "timeout fallback: $t is on PATH"; continue; }
    ln -s "$p" "$farm/$t"
done
ln -s "$hook_bin" "$farm/bash"
ln -s "$work/bin/cargo" "$farm/cargo"
ln -s "$work/bin/rustfmt" "$farm/rustfmt"
farm_stop() { : > "$work/calls"; out=$(printf '{"session_id":"%s"}' "$session" | PATH="$farm" "$hook_bin" "$hook" stop 2>&1); }
if PATH="$farm" "$hook_bin" -c 'command -v timeout || command -v gtimeout' >/dev/null; then
    bad "timeout fallback: the farm PATH has no timeout or gtimeout"
else
    ok "timeout fallback: the farm PATH has no timeout or gtimeout"
fi
mark "$a/src/lib.rs"; farm_stop
check "no timeout(1), no gtimeout: clippy still runs" cargo_ran_in "$a"
check "no timeout(1), no gtimeout: state cleared" test ! -e "$state"
printf '#!/usr/bin/env bash\necho "gtimeout $1" >> "%s/calls"\nshift\nexec "$@"\n' "$work" > "$farm/gtimeout"
chmod +x "$farm/gtimeout"
mark "$a/src/lib.rs"; farm_stop
check "gtimeout only: clippy runs under gtimeout" grep -qxF 'gtimeout 540' "$work/calls"
check "gtimeout only: clippy runs" cargo_ran_in "$a"

if [ "$fails" -eq 0 ]; then echo "all rust-check hold tests passed (hook shell bash $hook_ver)"; else echo "$fails rust-check hold test(s) failed (hook shell bash $hook_ver)"; exit 1; fi
