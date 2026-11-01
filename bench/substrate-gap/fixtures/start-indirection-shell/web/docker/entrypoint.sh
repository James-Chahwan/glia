#!/bin/sh
set -e

# Migrations run before the server: not the container's process.
node build/migrate.js

exec "$@"
