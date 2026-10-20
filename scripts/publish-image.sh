#!/usr/bin/env bash
# Build the fork image and push it to the Tencent CCR repository.
# Requires an existing `docker login ccr.ccs.tencentyun.com`.
# The version tag is immutable. `latest` moves to the image just pushed.
set -euo pipefail

ROOT_DIR=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
IMAGE_REPO="${IMAGE_REPO:-ccr.ccs.tencentyun.com/deali/agent-wechat}"
ARCH="amd64"
TAG=""
PUSH_LATEST=1

usage() {
  cat <<'EOF'
Usage: scripts/publish-image.sh [--tag <version>] [--arch amd64|arm64] [--no-latest]

Builds one architecture and pushes it to IMAGE_REPO
(default: ccr.ccs.tencentyun.com/deali/agent-wechat).

The default tag is the newest local v*-iugam.* tag with the leading v removed.
The script refuses to overwrite that version tag. It also refuses when docker/
or packages/agent-server-rust differ from that tag, including untracked files.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --tag)
      TAG="${2:-}"
      shift 2
      ;;
    --arch)
      ARCH="${2:-}"
      shift 2
      ;;
    --no-latest)
      PUSH_LATEST=0
      shift
      ;;
    -h|--help)
      usage
      exit 0
      ;;
    *)
      echo "unknown argument: $1" >&2
      usage >&2
      exit 1
      ;;
  esac
done

case "$ARCH" in
  amd64|arm64) ;;
  *)
    echo "unsupported arch: $ARCH (this publisher pushes one of amd64 or arm64)" >&2
    exit 1
    ;;
esac

if [ -z "$TAG" ]; then
  TAG=$(git -C "$ROOT_DIR" tag -l 'v*-iugam.*' --sort=-v:refname | head -1 || true)
  TAG="${TAG#v}"
fi
if [ -z "$TAG" ]; then
  echo "no v*-iugam.* tag found. Pass --tag." >&2
  exit 1
fi
if ! git -C "$ROOT_DIR" rev-parse -q --verify "refs/tags/v${TAG}" >/dev/null; then
  echo "tag v${TAG} does not exist. Publish moves a built tag, it does not invent one." >&2
  exit 1
fi

dirty=$(git -C "$ROOT_DIR" status --porcelain -- docker packages/agent-server-rust)
source_diff=$(git -C "$ROOT_DIR" diff --stat "v${TAG}" -- docker packages/agent-server-rust)
if [ -n "$dirty" ] || [ -n "$source_diff" ]; then
  echo "docker inputs differ from v${TAG}. Tag a new iugam revision before publishing." >&2
  printf '%s\n' "$dirty" "$source_diff" >&2
  exit 1
fi

REVISION=$(git -C "$ROOT_DIR" rev-parse "v${TAG}^{}")
REF="${IMAGE_REPO}:${TAG}"

echo "==> Checking ${REF}"
inspect_out=$(docker buildx imagetools inspect "$REF" 2>&1) && inspect_code=0 || inspect_code=$?
if [ "$inspect_code" -eq 0 ]; then
  echo "refusing to overwrite existing ${REF}" >&2
  exit 1
fi
if ! printf '%s' "$inspect_out" | grep -qi 'not found'; then
  echo "could not confirm ${REF} is unpublished:" >&2
  printf '%s\n' "$inspect_out" >&2
  exit 1
fi

echo "==> Building ${REF} (linux/${ARCH}) from v${TAG} (${REVISION})"
export BUILDKIT_PROGRESS=plain
bash "$ROOT_DIR/scripts/build-images-local.sh" --arch "$ARCH"

LOCAL_TAG="agent-wechat:${ARCH}"
docker tag "$LOCAL_TAG" "$REF"
echo "==> Pushing ${REF}"
docker push "$REF"

if [ "$PUSH_LATEST" -eq 1 ]; then
  LATEST="${IMAGE_REPO}:latest"
  docker tag "$LOCAL_TAG" "$LATEST"
  echo "==> Pushing ${LATEST}"
  docker push "$LATEST"
fi

echo "==> Pushed ${REF}"
docker buildx imagetools inspect "$REF"
