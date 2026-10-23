#!/usr/bin/bash
# Builds the image on Toolforge. Does not (re)start anything: until cutover,
# the webservice is still the PHP one.
set -euo pipefail
toolforge build start https://github.com/magnusmanske/magnustools_rs
