#!/usr/bin/env bash
#
# release-fields.sh -- add the obs release fields (obs#1 point 2) to a bundle manifest made by
# genlock-manifest.sh, as one "release" line right after the opening brace. genlock-manifest.sh
# --check reads the files[] entries line by line, so the rest of the file keeps its exact layout;
# the caller re-checks it against its stage.
#
# Both platforms write one schema: linux-genlock.yml (strih: vstack + CBJXS fields, the loudness
# dock) and windows-genlock.yml (no vstack checkout, no loudness dock: those fields are left out).
#
# Usage: release-fields.sh <manifest> <OBS_RELEASE.txt> [--loudness] [--vstack <dir> <tag> <commit>]
# Run from the repo root.
set -euo pipefail

if [ "$#" -lt 2 ]; then
	echo "usage: $0 <manifest> <OBS_RELEASE.txt> [--loudness] [--vstack <dir> <tag> <commit>]" >&2
	exit 2
fi
manifest="$1"
release_txt="$2"
shift 2
with_loudness=false
vstack_dir=""
vstack_tag=""
vstack_commit=""
while [ "$#" -gt 0 ]; do
	case "$1" in
	--loudness) with_loudness=true; shift ;;
	--vstack)
		if [ "$#" -lt 4 ]; then echo "ERROR: --vstack needs <dir> <tag> <commit>" >&2; exit 2; fi
		vstack_dir="$2"; vstack_tag="$3"; vstack_commit="$4"; shift 4 ;;
	*) echo "ERROR: unknown argument '$1'" >&2; exit 2 ;;
	esac
done

need() {
	if [ -z "$2" ] || [ "$2" = "null" ]; then echo "ERROR: could not read $1" >&2; exit 1; fi
}

stats="$(sed -n 's/^#define OBS_GENLOCK_STATS_VERSION \([0-9]\+\)$/\1/p' vendor/obs-studio/libobs/obs.h)"
out_stats="$(sed -n 's/^#define OBS_GENLOCK_OUTPUT_STATS_VERSION \([0-9]\+\)$/\1/p' vendor/obs-studio/libobs/obs.h)"
vban="$(sed -n 's/^project(obs-vban VERSION \([0-9.]\+\))$/\1/p' vendor/obs-vban/CMakeLists.txt)"
dock="$(sed -n 's/^project(obs-audio-video-sync-dock VERSION \([0-9.]\+\))$/\1/p' vendor/av-sync-dock/CMakeLists.txt)"
obs="$(jq -r '.components[] | select(.name == "obs-studio") | .pinned_version' "$manifest")"
distroav="$(jq -r '.components[] | select(.name == "distroav") | .pinned_version' "$manifest")"
tag="$(sed -n 's/^tag=//p' "$release_txt")"
commit="$(sed -n 's/^commit=//p' "$release_txt")"
version="$(sed -n 's/^version=//p' "$release_txt")"
for v in stats out_stats vban dock obs distroav tag commit version; do need "$v" "${!v}"; done

versions="$(jq -cn --arg obs "$obs" --arg distroav "$distroav" --arg vban "$vban" --arg dock "$dock" \
	'{"obs-studio": $obs, distroav: $distroav, "obs-vban": $vban, "av-sync-dock": $dock}')"
if $with_loudness; then
	loudness="$(sed -n 's/^project(loudness-dock VERSION \([0-9.]\+\))$/\1/p' vendor/obs-loudness-dock/CMakeLists.txt)"
	need loudness "$loudness"
	versions="$(jq -c --arg l "$loudness" '. + {"loudness-dock": $l}' <<<"$versions")"
fi

release="$(jq -cn \
	--arg tag "$tag" --arg commit "$commit" --arg version "$version" \
	--argjson versions "$versions" \
	--argjson stats "$stats" --argjson out_stats "$out_stats" \
	--arg obs_h "$(sha256sum vendor/obs-studio/libobs/obs.h | cut -d' ' -f1)" \
	'{obs_tag: $tag, obs_commit: $commit, obs_version: $version, versions: $versions,
	  genlock_stats_version: $stats, genlock_output_stats_version: $out_stats, obs_h_sha256: $obs_h}')"

if [ -n "$vstack_dir" ]; then
	abi="$(grep -rhoE '#define[[:space:]]+CBJXS_ABI_VERSION[[:space:]]+[0-9]+' "$vstack_dir/jxs/recv-ffi/include" "$vstack_dir/jxs/send-ffi/include" | awk '{print $3}' | sort -u)"
	if [ "$(printf '%s\n' "$abi" | grep -c .)" != 1 ]; then echo "ERROR: want one CBJXS_ABI_VERSION in the vstack headers, got: '$abi'" >&2; exit 1; fi
	need vstack_tag "$vstack_tag"
	need vstack_commit "$vstack_commit"
	release="$(jq -c --arg t "$vstack_tag" --arg c "$vstack_commit" --argjson abi "$abi" \
		'. + {vstack_tag: $t, vstack_commit: $c, cbjxs_abi_version: $abi}' <<<"$release")"
fi

awk -v line="  \"release\": $release," 'NR == 1 {print; print line; next} {print}' "$manifest" > "$manifest.tmp"
mv "$manifest.tmp" "$manifest"
jq -e '.release.obs_commit and (.files | length > 0)' "$manifest" > /dev/null
jq '.release' "$manifest"
