#!/bin/sh
#
# check for a polkadot companion pr and ensure it has approvals and is 
# mergeable
#

github_api_substrate_pull_url="https://api.github.com/repos/paritytech/substrate/pulls"
# use github api v3 in order to access the data without authentication
github_header="Accept: application/vnd.github.v3+json" 

boldprint () { printf "|\n| \033[1m${@}\033[0m\n|\n" ; }
boldcat () { printf "|\n"; while read l; do printf "| \033[1m${l}\033[0m\n"; done; printf "|\n" ; }



boldcat <<-EOT


check_polkadot_companion_status
===============================

this job checks if there is a string in the description of the pr like

polkadot companion: paritytech/polkadot#567

or any other polkadot pr is mentioned in this pr's description and checks its 
status.


EOT



# polkadot:master
if expr match "${CI_COMMIT_REF_NAME}" '^[0-9]\+$' >/dev/null
then
  boldprint "this is pull request no ${CI_COMMIT_REF_NAME}"
  # get the last reference to a pr in polkadot
  pr_body="$(curl -H "${github_header}" -s ${github_api_substrate_pull_url}/${CI_COMMIT_REF_NAME} \
    | sed -n -r 's/^[[:space:]]+"body": (".*")[^"]+$/\1/p')"

  pr_companion="$(echo "${pr_body}" | sed -n -r \
      -e 's;^.*polkadot companion: paritytech/polkadot#([0-9]+).*$;\1;p' \
      -e 's;^.*polkadot companion: https://github.com/paritytech/polkadot/pull/([0-9]+).*$;\1;p' \
    | tail -n 1)"
  if [ -z "${pr_companion}" ]
  then
    pr_companion="$(echo "${pr_body}" | sed -n -r \
      's;^.*https://github.com/paritytech/polkadot/pull/([0-9]+).*$;\1;p' \
      | tail -n 1)"
  fi

  if [ "${pr_companion}" ]
  then
    boldprint "companion pr specified/detected: #${pr_companion}"
    git fetch --depth 1 origin refs/pull/${pr_companion}/head:pr/${pr_companion}
    git checkout pr/${pr_companion}
  else
    boldprint "no companion pr found - building polkadot:master"
  fi
else
  boldprint "this is not a pull request - building polkadot:master"
fi

# Make sure we override the crates in native and wasm build
# patching the git path as described in the link below did not test correctly
# https://doc.rust-lang.org/cargo/reference/overriding-dependencies.html
mkdir .cargo
echo "paths = [ \"$SUBSTRATE_PATH\" ]" > .cargo/config

mkdir -p target/debug/wbuild/.cargo
cp .cargo/config target/debug/wbuild/.cargo/config

# package, others are updated along the way.
cargo update

# Test Polkadot pr or master branch with this Substrate commit.
time cargo test --all --release --verbose

