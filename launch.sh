#!/usr/bin/env bash
set -e
cd /Users/sarwa/Ingest

if [ ! -d "node_modules" ]; then
  echo "Installing dependencies (first run — ~1 min)..."
  npm install
  echo ""
fi

npm start
