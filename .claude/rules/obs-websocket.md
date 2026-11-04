---
paths:
  - "vendor/obs-studio/plugins/obs-websocket/src/**"
  - "tests/obs-websocket-graphics/**"
---

# obs-websocket graphics

- Requests run on the plugin's Qt thread pool. A pool thread ends after 30 s idle, so most requests run on a fresh thread.
- Never call `obs_enter_graphics()` from request code. A fresh thread's first GL MakeCurrent (~60 ms on strih) holds the graphics lock, and the render tick drops a JPEG XS frame (camera-box#1387).
- Put GL work in `src/utils/Obs_GraphicsTask.cpp` through `RunGraphicsTask`. It runs the work as a waiting `OBS_TASK_GRAPHICS` task on the graphics thread, or inline when the caller already holds graphics.
- Keep `Obs_GraphicsTask.cpp` libobs-only, with no Qt and no plugin headers. `tests/obs-websocket-graphics` compiles it alone against obs-sdk. The plugin cannot load without a frontend main window.
