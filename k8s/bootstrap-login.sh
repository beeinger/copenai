#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
NS="${COPENAI_K8S_NAMESPACE:-copenai}"
DEPLOY="${ROOT}/deployment-login.yaml"

need() {
  command -v "$1" >/dev/null 2>&1 || {
    echo "error: $1 not found" >&2
    exit 1
  }
}

need kubectl

echo "Applying ${DEPLOY} ..."
kubectl apply -f "${DEPLOY}"

echo "Waiting for deployment ..."
kubectl rollout status deployment/copenai -n "${NS}" --timeout=180s

echo ""
echo "Cursor CLI login (open the URL in your browser when prompted)."
echo "Session is saved on the PVC at /data/.cursor and survives restarts."
echo ""
kubectl exec -it deployment/copenai -n "${NS}" -- copenai auth login

echo ""
echo "Auth status:"
kubectl exec -it deployment/copenai -n "${NS}" -- copenai auth status

if kubectl exec deployment/copenai -n "${NS}" -- copenai keys list 2>/dev/null | grep -q .; then
  echo ""
  echo "Wrapper API key already exists — skipping keys add."
else
  echo ""
  echo "Creating wrapper API key for OpenAI clients (secret shown once):"
  kubectl exec -it deployment/copenai -n "${NS}" -- copenai keys add --name default
fi

echo ""
echo "Health:"
kubectl exec deployment/copenai -n "${NS}" -- curl -fsS http://127.0.0.1:9241/health
echo ""
echo ""
echo "Port-forward example:"
echo "  kubectl port-forward -n ${NS} svc/copenai 9241:9241"
