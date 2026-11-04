/*
obs-websocket
Copyright (C) 2020-2021 Kyle Manning <tt2468@gmail.com>

This program is free software; you can redistribute it and/or modify
it under the terms of the GNU General Public License as published by
the Free Software Foundation; either version 2 of the License, or
(at your option) any later version.

This program is distributed in the hope that it will be useful,
but WITHOUT ANY WARRANTY; without even the implied warranty of
MERCHANTABILITY or FITNESS FOR A PARTICULAR PURPOSE.  See the
GNU General Public License for more details.

You should have received a copy of the GNU General Public License along
with this program. If not, see <https://www.gnu.org/licenses/>
*/

#pragma once

#include <stddef.h>
#include <stdint.h>
#include <obs.h>

// camera-box#1387 (obs#1): the GL work of obs-websocket requests. Requests run on the plugin's Qt
// thread pool; libobs-only on purpose, so tests/obs-websocket-graphics builds it without the plugin.
namespace Utils {
	namespace Obs {
		namespace GraphicsTask {
			// Renders source at imgWidth x imgHeight into dst (RGBA, dstLinesize bytes per row).
			bool RenderSourceRGBA(obs_source_t *source, uint32_t imgWidth, uint32_t imgHeight, uint8_t *dst,
					      size_t dstLinesize);
			// Adds source to scene. Returns the new item with a reference for the caller.
			obs_sceneitem_t *AddSceneItem(obs_scene_t *scene, obs_source_t *source, bool enabled,
						      const obs_transform_info *transform, const obs_sceneitem_crop *crop);
		}
	}
}
