#!/bin/bash

COUNT=${1:-1}

echo "Starting $COUNT node(s)..."

for i in $(seq 1 $COUNT); do
    echo "Starting node $i"
    cargo run --release -p node-svc &
done

echo "All $COUNT node(s) started in background"