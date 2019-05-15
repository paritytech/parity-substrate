#!/bin/bash

DOWNLOAD_URL="https://releases.parity.io/substrate/x86_64-debian:stretch/2.0.0-${CI_COMMIT_SHORT_SHA}/substrate"
POST_DATA='{"extra_vars":{"artifact_path":"'${DOWNLOAD_URL}'"}}'

wget -O -  --header "Authorization: Bearer ${AWX_TOKEN}" --header "Content-type: application/json" --post-data "${POST_DATA}" https://ansible-awx.parity.io/api/v2/job_templates/32/launch/
