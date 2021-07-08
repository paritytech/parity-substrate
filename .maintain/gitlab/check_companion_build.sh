#!/usr/bin/env bash
#
# check if a pr is compatible with companion pr or master if not available
#
# to override one that was just mentioned mark companion pr in the body of the
# pr like
#
# $REPO companion: $ORGANISATION/$REPO#567
#
# $ORGANISATION and $REPO are set using $1 and $2. You can also specify a custom
# build command with $3
# Every argument after $3 is for specifying *additional* dependencies this
# project has that depend on substrate, which might also have companion PRs.

# Example: Cumulus relies on both substrate and polkadot. If this substrate PR
# requires a companion build on polkadot, when we are testing that cumulus builds
# with this commit of substrate, we *also* need to clone & patch polkadot, and tell
# cumulus to use this cloned+patched version, else the build is guaranteed to fail
# (since it doesn't have the changes to polkadot that were required in the polkadot
# companion PR)

# So invoke this script like (arguments in [] indicate optional arguments)
# ./check_companion_build.sh paritytech cumulus ['cargo test --release'] [paritytech/polkadot]

set -e

# Include the common functions library
#shellcheck source=../common/lib.sh
. "$(dirname "${0}")/../common/lib.sh"

ORGANISATION=$1
REPO=$2
BUILDSTRING=${3:-cargo test --workspace --release}
DEPS=("${@:4}")
SUBSTRATE_DIR="$(pwd)"

boldprint () { printf "|\n| \033[1m%s\033[0m\n|\n" "${@}"; }
boldcat () { printf "|\n"; while read -r l; do printf "| \033[1m%s\033[0m\n" "${l}"; done; printf "|\n" ; }

boldcat <<-EOT


check_${REPO}_companion_build
==============================

this job checks if there is a string in the description of the pr like

$REPO companion: $ORGANISATION/$REPO#567


it will then run cargo check from this ${REPO}'s branch with substrate code
from this pull request. otherwise, it will uses master instead


EOT

# Set the user name and email to make merging work
git config --global user.name 'CI system'
git config --global user.email '<>'

# Merge master into our branch before building to make sure we don't miss
# any commits that are required.
git fetch --depth 100 origin
git merge origin/master

# Clone the current master branch into a local directory
# NOTE: we need to pull enough commits to be able to find a common
# ancestor for successfully performing merges below.
git clone --depth 20 "https://github.com/${ORGANISATION}/${REPO}.git"

cd "$REPO"

# either it's a pull request then check for a companion otherwise use
# master
# shellcheck disable=SC2003
if expr match "${CI_COMMIT_REF_NAME}" '^[0-9]\+$' >/dev/null
then
  boldprint "this is pull request no ${CI_COMMIT_REF_NAME}"
  pr_companion="$(get_companion "paritytech/substrate" "$CI_COMMIT_REF_NAME" "$ORGANISATION/$REPO")"
  if [ "$pr_companion" ]
  then
    boldprint "companion pr specified/detected: #${pr_companion}"
    git fetch origin "refs/pull/${pr_companion}/head:pr/${pr_companion}"
    git checkout "pr/${pr_companion}"
    git merge origin/master
  else
    boldprint "no companion branch found - building ${REPO}:master"
  fi

  # If this repo has any additional dependencies, we should check whether they
  # are mentioned as companions as well. If they are, we patch this repo to
  # use that companion build as well. See example at top of this script
  # Note: Will only work with repos supported by diener
  declare -A diener_commands
  diener_commands=()
  diener_commands["paritytech/polkadot"]='--polkadot'
  diener_commands["paritytech/substrate"]='--substrate'
  diener_commands["paritytech/grandpa-bridge-gadget"]='--beefy'

  for dep in "${DEPS[@]}"; do
    dep_companion="$(get_companion "paritytech/substrate" "$CI_COMMIT_REF_NAME" "$dep")"
    if [ "$dep_companion" ]; then
      echo "Companion PR found for $dep, need to patch $REPO to use that"
      git clone --depth 20 "https://github.com/$dep.git" "$dep"
      git -C "$dep" fetch origin "refs/pull/${dep_companion}/head:pr/${dep_companion}"
      git -C "$dep" checkout "pr/${dep_companion}"
      git -C "$dep" merge origin/master
      # Tell this dependency to use the version of substrate in this PR
      diener patch --crates-to-patch "$SUBSTRATE_DIR" --substrate --path "$dep/Cargo.toml"
      # then we tell this repository to point at our locally cloned dependency
      diener patch --crates-to-patch "$dep" "${diener_commands[$dep]}" --path "Cargo.toml"
    fi

  done

else
  boldprint "this is not a pull request - building ${REPO}:master"
fi

# Test pr or master branch with this Substrate commit.
diener patch --crates-to-patch ".." --substrate --path "Cargo.toml"

eval "$BUILDSTRING"
