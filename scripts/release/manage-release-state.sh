#!/usr/bin/env bash
# Inspect release state or create one immutable tag. All API errors except an
# explicit GitHub 404 fail closed; tags are never force-updated or deleted.
set -euo pipefail

command_name=${1:?usage: manage-release-state.sh <inspect|ensure-tag> <tag> <sha>}
tag=${2:?usage: manage-release-state.sh <inspect|ensure-tag> <tag> <sha>}
sha=${3:?usage: manage-release-state.sh <inspect|ensure-tag> <tag> <sha>}
: "${GH_REPO:?GH_REPO is required}"
: "${GH_TOKEN:?GH_TOKEN is required}"

api_value() {
  local endpoint=$1 filter=$2 error value
  error=$(mktemp)
  if value=$(gh api "$endpoint" --jq "$filter" 2>"$error"); then
    rm -f "$error"
    printf '%s' "$value"
    return 0
  fi
  if grep -Eq 'HTTP[^[:alnum:]]*404' "$error"; then
    rm -f "$error"
    return 0
  fi
  cat "$error" >&2
  rm -f "$error"
  return 1
}

tag_sha=$(api_value "repos/$GH_REPO/git/ref/tags/$tag" '.object.sha')
if [[ -z $tag_sha ]]; then
  tag_exists=false
else
  [[ $tag_sha == "$sha" ]] || {
    printf 'existing tag "%s" points to %s, expected frozen SHA %s\n' \
      "$tag" "$tag_sha" "$sha" >&2
    exit 1
  }
  tag_exists=true
fi

release_draft=$(api_value "repos/$GH_REPO/releases/tags/$tag" '.draft')
if [[ -z $release_draft ]]; then
  release_state=none
elif [[ $release_draft == true ]]; then
  release_state=draft
else
  printf 'published GitHub Release already exists for tag "%s"; refusing overwrite\n' \
    "$tag" >&2
  exit 1
fi

case $command_name in
inspect)
  printf 'tag-exists=%s\nrelease-state=%s\n' "$tag_exists" "$release_state"
  ;;
ensure-tag)
  [[ $release_state == none || $release_state == draft ]] || exit 1
  if [[ $tag_exists == false ]]; then
    if ! gh api --method POST "repos/$GH_REPO/git/refs" \
      -f "ref=refs/tags/$tag" -f "sha=$sha"; then
      # A request can succeed server-side while the runner observes a transport
      # failure. Re-read and continue only if immutable state is now correct.
      after_failure=$(api_value "repos/$GH_REPO/git/ref/tags/$tag" '.object.sha')
      [[ $after_failure == "$sha" ]] || {
        printf 'tag creation failed and immutable tag was not observed at %s\n' "$sha" >&2
        exit 1
      }
    fi
  fi
  verified_sha=$(api_value "repos/$GH_REPO/git/ref/tags/$tag" '.object.sha')
  [[ $verified_sha == "$sha" ]] || {
    printf 'tag "%s" verification failed: got %s, expected %s\n' \
      "$tag" "${verified_sha:-<missing>}" "$sha" >&2
    exit 1
  }
  printf 'tag=%s\nsha=%s\n\n' "$tag" "$verified_sha"
  ;;
*)
  printf 'unknown command: %s\n' "$command_name" >&2
  exit 2
  ;;
esac
