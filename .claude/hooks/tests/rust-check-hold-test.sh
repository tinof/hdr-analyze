#!/usr/bin/env bash
# Tests for the builder hold of ../rust-check.sh, in a throwaway repository with a linked worktree
# and stub `cargo` and `rustfmt` on PATH (no real build runs). While a fresh
# <git dir>/claude-builder.<run>.hold exists, `stop` neither formats nor lints that checkout and
# keeps its touched files; a stale hold does not count; two runs do not clear each other's hold;
# a hold in one checkout does not hold another, also for a worktree nested inside the main
# checkout; `mark` renews the holds of the checkout it edits.
#
# Usage: .claude/hooks/tests/rust-check-hold-test.sh
set -u
here=$(cd "$(dirname "$0")" && pwd)
hook="$here/../rust-check.sh"
work=$(mktemp -d "${TMPDIR:-/tmp}/rust-check-hold-test.XXXXXX")
trap 'rm -rf "$work"' EXIT
fails=0
ok() { echo "ok   $1"; }
bad() { echo "FAIL $1"; fails=$((fails + 1)); }
check() { local name=$1; shift; if "$@"; then ok "$name"; else bad "$name"; fi; }

# Stubs: record every call with the directory it ran in (rustfmt also with its arguments, so a
# test sees which files it formatted), print nothing, succeed.
mkdir -p "$work/bin" "$work/tmp"
printf '#!/usr/bin/env bash\necho "cargo $PWD" >> "%s/calls"\n' "$work" > "$work/bin/cargo"
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
mark() { printf '{"session_id":"%s","tool_input":{"file_path":"%s"}}' "$session" "$1" | bash "$hook" mark; }
stop() { : > "$work/calls"; out=$(printf '{"session_id":"%s"}' "$session" | bash "$hook" stop 2>&1); }
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

if [ "$fails" -eq 0 ]; then echo "all rust-check hold tests passed"; else echo "$fails rust-check hold test(s) failed"; exit 1; fi
