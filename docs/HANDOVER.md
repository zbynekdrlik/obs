# Handover: obs (2026-11-02)

## Status

This project owns the forked OBS. It splits from `zbynekdrlik/camera-box` under ticket airuleset#1280. The repository starts empty on purpose.

The tree arrives from camera-box only after camera-box extracts the JPEG XS crate, so that no OBS build depends on the whole camera-box crate. camera-box reports that to airuleset. Do not copy `vendor/` by hand before that: the import keeps history and the `vendor/` paths.

## Agreed with camera-box

- The JPEG XS crate moves here first.
- Parity runs through versioned test vectors that camera-box publishes.
- `vendor/` paths stay as they are.
- The Dante OBS source (camera-box#1485) is written here, not in camera-box. This is the owner's decision of 2026-11-02, relayed by camera-box.

## What else acts on the machines

- camera-box: the picture, the boxes, the deploy. It pins your released OBS builds. You do not deploy to the boxes yourself.
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
