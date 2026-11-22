#!/usr/bin/env bash
# Delopy and restart toolforge services
ssh login.toolforge.org 'become magnustools bash -lc "cd ~/magnustools_rs/ && git pull && ./build.sh && webservice restart && echo \"OK\""'
