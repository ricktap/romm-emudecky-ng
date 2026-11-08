#!/bin/sh
# Container entry point for the Decky CLI backend build (see Dockerfile).
set -e

cd /backend

make
