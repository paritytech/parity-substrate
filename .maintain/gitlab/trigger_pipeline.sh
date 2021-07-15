#!/bin/bash

set -eou pipefail

# This script is to trigger Simnet pipeline.
# See help article for more details.

SCRIPT_NAME="$0"
SCRIPT_PATH=$(dirname "$0")               # relative
SCRIPT_PATH=$(cd "${SCRIPT_PATH}" && pwd) # absolutized and normalized
SIMNET_VERSION=""
FEATURES=""

function usage {
  cat << EOF
This script is to trigger Simnet pipeline.
It's designed to be launched locally and from CI.
The required argumants for both cases are listed below.

Usage: ${SCRIPT_NAME} OPTION

OPTIONS

    -h, --help                  Print this help message.

  Mandatory in both cases:

    -s, --simnet-version        Simnet version to trigger.
                                E.g.: v6

    -u, --upstream-project      Triggering project.
                                E.g.: substrate

    -r, --upstream-ref          The branch or tag name for which project is built.
                                E.g.: master

    -d, --downstream-id         Downstream project's ID to trigger.
                                E.g.: 332 (simnet project id)

    -n, --image-name            Name of image to test.
                                E.g.: docker.io/parity/substrate

    -i, --image-tag             Tag of the image to test.
                                E.g.: latest

    -f, --features              Path to cucumber features dir or file.This path is 
                                relative to runt_tests.sh script specified 
                                as arg to "--test-script"
                                E.g.: tests/quick

  Required for local launch:

    -g, --ci-server-fqdn        FQDN of your gitlab server.
                                E.g.: gitlab.parity.io

    -t, --trigger-token         Gitlab trigger token. This must be defined in
                                project -> settings -> CI/CD -> Pipeline triggers
                                Defaults to CI_JOB_TOKEN
                                https://stackoverflow.com/questions/42746634/gitlab-trigger-api-returns-404

    -a, --access-token          Gitlab  peronal access token or it defaults to
                                PIPELINE_TOKEN (gitlab variable)
                                https://docs.gitlab.com/ee/user/profile/personal_access_tokens.html

EXAMPLES
  ${SCRIPT_NAME} -s v4
  ${SCRIPT_NAME} --simnet-version=v4

  Local test example. You need to set the 2 vars before running: TR_TOKEN and PERS_TOKEN
  ${SCRIPT_NAME} --simnet-version=v6 \\
                 --upstream-project=substrate \\
                 --upstream-ref=master \\
                 --image-name=docker.io/parity/substrate \\
                 --features=tests/quick \\
                 --image-tag=latest \\
                 --ci-server-fqdn=gitlab.parity.io \\
                 --downstream-id=332 \\
                 --trigger-token="\${TR_TOKEN}" \\
                 --access-token="\${PERS_TOKEN}"
EOF
}

function main {
  # Main entry point for the script.
  parse_args "$@"
  check_args
  trigger_pipeline
  check_pipeline
  poll_pipeline
}

function parse_args {
  # shellcheck disable=SC2214
  while getopts f:r:a:c:t:u:r:i:n:g:s:h-: OPT; do
    # support long options: https://stackoverflow.com/a/28466267/519360
    if [ "${OPT}" = "-" ]; then   # long option: reformulate OPT and OPTARG
      OPT="${OPTARG%%=*}"         # extract long option name
      OPTARG="${OPTARG#$OPT}"     # extract long option argument (may be empty)
      OPTARG="${OPTARG#=}"        # if long option argument, remove assigning `=`
    fi
    case "${OPT}" in
      h | help )                usage     ; exit 0 ;;
      s | simnet-version )      needs_arg ; SIMNET_VERSION="${OPTARG}" ;;
      u | upstream-project )    needs_arg ; TRGR_PROJECT="${OPTARG}" ;;
      r | upstream-ref )        needs_arg ; TRGR_REF="${OPTARG}" ;;
      n | image-name )          needs_arg ; IMAGE_NAME="${OPTARG}" ;;
      i | image-tag )           needs_arg ; IMAGE_TAG="${OPTARG}" ;;
      f | features )            needs_arg ; FEATURES="${OPTARG}" ;;
      g | ci-server-fqdn )      needs_arg ; CI_SERVER_HOST="${OPTARG}" ;;
      d | downstream-id )       needs_arg ; DWNSTRM_ID="${OPTARG}" ;;
      t | trigger-token )       needs_arg ; CI_JOB_TOKEN="${OPTARG}" ;;
      a | access-token )        needs_arg ; PIPELINE_TOKEN="${OPTARG}" ;;
      ??* )                     log DIE "Illegal option --${OPT}" ;;  # bad long option
      ? )                       exit 2 ;;  # bad short option (error reported via getopts)
    esac
  done
  shift $((OPTIND-1)) # remove parsed options and args from $@ list

}

function check_args {
  if [[ -z "${SIMNET_VERSION}" ]] ; then
    log DIE "Must specify value for mandatory argument -s,--simnet-version

$(usage)"
  fi
}

function needs_arg {
  if [ -z "${OPTARG}" ]; then
    log DIE "No arg for --${OPT} option"
  fi
}

function trigger_pipeline {
  # API trigger another project's pipeline.
  log INFO "Triggering Simnet pipeline."

  curl --silent \
      -X POST \
      -F "token=${CI_JOB_TOKEN}" \
      -F "ref=${SIMNET_VERSION}" \
      -F "variables[TRGR_PROJECT]=${TRGR_PROJECT}" \
      -F "variables[TRGR_REF]=${TRGR_REF}" \
      -F "variables[IMAGE_NAME]=${IMAGE_NAME}" \
      -F "variables[IMAGE_TAG]=${IMAGE_TAG}" \
      -F "variables[FEATURES]=${FEATURES}" \
      "https://${CI_SERVER_HOST}/api/v4/projects/${DWNSTRM_ID}/trigger/pipeline" | \
          tee pipeline;
}

function check_pipeline {
  PIPELINE_ID=$(jq ".id" pipeline)
  PIPELINE_URL=$(jq ".web_url" pipeline)
  echo
  log INFO "Simnet pipeline ${PIPELINE_URL} was successfully triggered."
  log INFO "Now we're polling it to obtain the distinguished status."
}

function poll_pipeline {
  # This is a workaround for a Gitlab bug, waits here until
  # https://gitlab.com/gitlab-org/gitlab/-/issues/326137 gets fixed.
  # The timeout is 360 curls with 8 sec interval, roughly an hour.
  log INFO "Waiting on ${PIPELINE_ID} status..."

# shellcheck disable=SC2034
  for i in {1..360}; do
      STATUS=$(get_status);
      log INFO "Triggered pipeline status is ${STATUS}";
      if [[ ${STATUS} =~ ^(pending|running|created)$ ]]; then
          echo;
      elif [[ ${STATUS} =~ ^(failed|canceled|skipped|manual)$ ]]; then
          log DIE "Something's broken in: ${PIPELINE_URL}";
      elif [[ ${STATUS} =~ ^(success)$ ]]; then
          log INFO "Look how green it is: ${PIPELINE_URL}"
          exit 0
      else
          log DIE "Something else has happened in ${PIPELINE_URL}"
      fi
  sleep 8;
  done
}

function get_status() {
    curl --silent \
        --header "PRIVATE-TOKEN: ${PIPELINE_TOKEN}" \
        "https://${CI_SERVER_HOST}/api/v4/projects/${DWNSTRM_ID}/pipelines/${PIPELINE_ID}" | \
            jq --raw-output ".status";
}

function log {
  local lvl msg fmt
  lvl=$1 msg=$2
  fmt='+%Y-%m-%d %H:%M:%S'
  lg_date=$(date "${fmt}")
  if [[ "${lvl}" = "DIE" ]] ; then
    lvl="ERROR"
    echo  "${lg_date} - ${lvl} - ${msg}"
    exit 1
  else
    echo "${lg_date} - ${lvl} - ${msg}"
  fi
}

main "$@"
