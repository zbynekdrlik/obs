# Handover: obs (2026-11-02)

## Status

This project owns the forked OBS. It split from `zbynekdrlik/camera-box` under ticket airuleset#1280, with history, from camera-box main 3d06cb46f8.

Imported: `vendor/obs-studio`, `vendor/distroav`, `vendor/av-sync-dock`, `vendor/obs-vban`, `vendor/README.md`, `vendor/BUILD.md`, `scripts/update-av-stack.sh`, `scripts/genlock-manifest.sh` and the four genlock workflows.

The tree does not build yet. It needs `jxs/*`, `vendor/svt-jpeg-xs` and `rig/core`, which moved to `zbynekdrlik/vstack`. First work: pin a vstack tag, make the build and the imported workflows run here. The Rust tests that touch only vendored source are listed in section C of camera-box#1487 and still live in camera-box.

## Agreed with camera-box

- JPEG XS and `rig/core` live in vstack (owner, 2026-11-02). Direction: vstack, then obs and the cambox app, then camera-box as orchestrator.
- Parity runs through versioned test vectors that camera-box publishes.
- `vendor/` paths stay as they are.
- The Dante OBS source (camera-box#1485) is written here, not in camera-box. This is the owner's decision of 2026-11-02, relayed by camera-box.

## What else acts on the machines

- camera-box: the picture, the boxes, the deploy. It pins your released OBS builds. You do not deploy to the boxes yourself.
- vstack: the shared video libraries. You pin its tags.
- dstack: all Dante code. A Dante need or bug goes there, and this project only moves the stack version.
- airuleset: accounts, tools, permissions.

songplayer, restreamer and others have no link here. They ask camera-box, so that no change reaches the fork unchecked and breaks the picture.

## Machines

- dev1: this project's account, with no sudo. Ask airuleset for build tools by peer.

## How to run a test

Nothing to run yet. The tree and its build arrive with the import; parity runs through the versioned vectors camera-box publishes.

## Site facts

Never commit a secret. Private facts go in `~/.config/obs/`.

## Runners

None registered. camera-box CI built this code before. The genlock workflows arrive with the import.

## Backlog

Owner-requested:

- Handle the forked OBS as its own project (2026-11-01).
- The Dante OBS source (camera-box#1485).

Proposed by a session, not yet asked for by the owner: nothing.

## Knowledge left in other projects

- camera-box: section C of camera-box#1487, `vendor/README.md`, `vendor/BUILD.md`, rule `obs-vban-receive.md`, camera-box#1485.
- airuleset: airuleset#1280.
