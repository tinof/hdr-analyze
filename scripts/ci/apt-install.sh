#!/usr/bin/env bash
# Install apt packages on a GitHub-hosted Ubuntu runner without hanging on a slow mirror.
#
# Usage: scripts/ci/apt-install.sh <archive dir> <package>...
#
# The runner's apt sources go through /etc/apt/apt-mirrors.txt. apt can stay on a mirror that
# sends a few kB/s for many minutes (actions/runner-images#14823: 68.6 MB in 8 min 30 s here),
# and `apt-get update` can go silent after a mirror failover (#15040); the image's own 15 s
# timeout only catches total stalls. So:
#   1. `apt-get update --error-on=any` and the package download (`--download-only`) run under a
#      wall-clock limit. `--error-on=any` turns a partial update into a failure, so the retry
#      below runs instead of installing from stale lists.
#   2. On a timeout or failure, the first mirror line (the one apt tries first) is dropped from
#      apt-mirrors.txt and both run again. A last attempt uses a plain `apt-get update`, so a
#      third-party source with a lasting error (packages.microsoft.com) cannot fail CI alone.
#   3. dpkg installs from the downloaded files (`--no-download`) with no time limit: killing
#      dpkg while it configures a package would break the retry.
# The .deb files go to <archive dir>, which the apt-install composite action restores from and
# saves to the Actions cache. apt checks each cached file against the fresh lists by hash and
# downloads only what is missing, so a warm cache leaves only the package lists to the mirrors.
set -uo pipefail

archives=${1:?usage: $0 <archive dir> <package>...}
shift
(($# > 0)) || { echo "usage: $0 <archive dir> <package>..." >&2; exit 2; }

mirrors=/etc/apt/apt-mirrors.txt
opts=(-y --no-install-recommends -o "Dir::Cache::Archives=$archives")
mkdir -p "$archives/partial"

# The download phase of one attempt. $1 is extra `apt-get update` options. A healthy mirror
# delivers the largest set (68.6 MB) in 1-4 s; the limits still allow about 1 MB/s. A stall that
# ran into the old 240 s limit cost a job 4 minutes before the retry, which then took 6 s.
fetch() {
    # shellcheck disable=SC2086 # $1 is a word list on purpose
    sudo timeout -k 5 60 apt-get update $1 \
        && sudo timeout -k 5 90 apt-get install --download-only "${opts[@]}" "${@:2}"
}

ok=0
for attempt in 1 2 3; do
    update_opts=--error-on=any
    if ((attempt == 2)); then
        if [[ -f $mirrors ]] && (($(grep -c . "$mirrors") > 1)); then
            echo "::warning::apt attempt 1 failed or timed out; dropping the first mirror: $(head -n 1 "$mirrors")"
            sudo sed -i '1d' "$mirrors"
        else
            echo "::warning::apt attempt 1 failed or timed out; retrying"
        fi
    elif ((attempt == 3)); then
        echo "::warning::apt attempt 2 failed or timed out; last attempt with a plain apt-get update"
        update_opts=
    fi
    start=$SECONDS
    if fetch "$update_opts" "$@"; then
        echo "::notice::apt download for '$*': attempt $attempt, $((SECONDS - start)) s"
        ok=1
        break
    fi
    echo "apt download attempt $attempt failed after $((SECONDS - start)) s"
done
((ok)) || { echo "::error::apt download failed after 3 attempts: $*"; exit 1; }

start=$SECONDS
sudo apt-get install --no-download "${opts[@]}" "$@" || exit 1
echo "apt install from the downloaded files: $((SECONDS - start)) s"

# actions/cache runs as the runner user and must read every file; the lock and partial
# downloads are not worth caching.
sudo rm -rf "$archives/lock" "$archives/partial"
sudo chown -R "$(id -u):$(id -g)" "$archives"
