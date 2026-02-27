#!/usr/bin/env bash
# Launch cmx-agent on all Slurm-allocated nodes.
#
# Usage:
#   salloc -N 4 bash deploy/slurm/launch_agents.sh
#
# Or within an sbatch script:
#   srun bash deploy/slurm/launch_agents.sh

set -euo pipefail

CMX_BINARY="${CMX_BINARY:-/usr/local/bin/cmx-agent}"
CMX_CONFIG="${CMX_CONFIG:-/etc/cmx/cmx-agent.toml}"
CMX_ETCD="${CMX_ETCD_ENDPOINTS:-http://localhost:2379}"
CMX_PORT="${CMX_PORT:-50051}"
CMX_MODEL_ID="${CMX_MODEL_ID:-}"

NODE_ID="$(hostname)"

echo "[cmx] Starting cmx-agent on ${NODE_ID} (port ${CMX_PORT})"

export CMX_NODE_ID="${NODE_ID}"
export CMX_LISTEN_ADDR="0.0.0.0:${CMX_PORT}"
export CMX_ETCD_ENDPOINTS="${CMX_ETCD}"
export CMX_MODEL_ID="${CMX_MODEL_ID}"

exec "${CMX_BINARY}" --config "${CMX_CONFIG}"
