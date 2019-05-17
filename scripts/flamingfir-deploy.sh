#!/bin/bash

TIMEOUT=60
TARGET_HOST="$1"
COMMIT=$(echo ${CI_BUILD_REF} | cut -c -9)
DOWNLOAD_URL="https://releases.parity.io/substrate/x86_64-debian:stretch/2.0.0-${COMMIT}/substrate"
POST_DATA='{"extra_vars":{"artifact_path":"'${DOWNLOAD_URL}'","target_host":"'${TARGET_HOST}'"}}'

JOB_ID=$(wget -O - --header "Authorization: Bearer ${AWX_TOKEN}" --header "Content-type: application/json" --post-data "${POST_DATA}" https://ansible-awx.parity.io/api/v2/job_templates/32/launch/ | jq .job)

echo "Launched job: $JOB_ID"

sleep $TIMEOUT

AWX_OUTPUT=$(wget -O - --header "Authorization: Bearer ${AWX_TOKEN}"  https://ansible-awx.parity.io/api/v2/jobs/${JOB_ID}/stdout?format=txt_download)

echo "AWX job log:"
echo "$AWX_OUTPUT"


JOB_STATUS=$(wget -O - --header "Authorization: Bearer ${AWX_TOKEN}"  https://ansible-awx.parity.io/api/v2/jobs/${JOB_ID}/ | jq .status )

echo "==================================="
echo -e "Ansible AWX Remote Job: ${JOB_ID} \x1B[31mStatus: ${JOB_STATUS}\x1B[0m"
echo "==================================="
