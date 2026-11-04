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

#include <string.h>
#include <graphics/graphics.h>
#include <graphics/vec4.h>

#include "Obs_GraphicsTask.h"

static void RunGraphicsTask(void (*task)(void *), void *param)
{
	obs_enter_graphics();
	task(param);
	obs_leave_graphics();
}

struct RenderData {
	obs_source_t *source;
	uint32_t width;
	uint32_t height;
	uint8_t *dst;
	size_t dstLinesize;
	bool success;
};

static void RenderTask(void *param)
{
	auto *d = static_cast<RenderData *>(param);
	const uint32_t sourceWidth = obs_source_get_width(d->source);
	const uint32_t sourceHeight = obs_source_get_height(d->source);
	const size_t rowBytes = (size_t)d->width * 4;

	gs_texrender_t *texRender = gs_texrender_create(GS_RGBA, GS_ZS_NONE);
	gs_stagesurf_t *stageSurface = gs_stagesurface_create(d->width, d->height, GS_RGBA);

	d->success = false;
	gs_texrender_reset(texRender);
	if (gs_texrender_begin(texRender, d->width, d->height)) {
		vec4 background;
		vec4_zero(&background);

		gs_clear(GS_CLEAR_COLOR, &background, 0.0f, 0);
		gs_ortho(0.0f, (float)sourceWidth, 0.0f, (float)sourceHeight, -100.0f, 100.0f);

		gs_blend_state_push();
		gs_blend_function(GS_BLEND_ONE, GS_BLEND_ZERO);

		obs_source_inc_showing(d->source);
		obs_source_video_render(d->source);
		obs_source_dec_showing(d->source);

		gs_blend_state_pop();
		gs_texrender_end(texRender);

		uint8_t *videoData = nullptr;
		uint32_t videoLinesize = 0;
		gs_stage_texture(stageSurface, gs_texrender_get_texture(texRender));
		if (gs_stagesurface_map(stageSurface, &videoData, &videoLinesize)) {
			for (uint32_t y = 0; y < d->height; y++)
				memcpy(d->dst + y * d->dstLinesize, videoData + (size_t)y * videoLinesize, rowBytes);
			gs_stagesurface_unmap(stageSurface);
			d->success = true;
		}
	}

	gs_stagesurface_destroy(stageSurface);
	gs_texrender_destroy(texRender);
}

bool Utils::Obs::GraphicsTask::RenderSourceRGBA(obs_source_t *source, uint32_t imgWidth, uint32_t imgHeight,
						uint8_t *dst, size_t dstLinesize)
{
	if (!source || !imgWidth || !imgHeight || !dst || dstLinesize < (size_t)imgWidth * 4)
		return false;

	RenderData d = {source, imgWidth, imgHeight, dst, dstLinesize, false};
	RunGraphicsTask(RenderTask, &d);
	return d.success;
}

struct AddItemData {
	obs_scene_t *scene;
	obs_source_t *source;
	bool enabled;
	const obs_transform_info *transform;
	const obs_sceneitem_crop *crop;
	obs_sceneitem_t *item;
};

static void AddItemHelper(void *param, obs_scene_t *scene)
{
	auto *d = static_cast<AddItemData *>(param);

	d->item = obs_scene_add(scene, d->source);
	if (!d->item)
		return;
	// The caller's reference, taken while the scene is still locked.
	obs_sceneitem_addref(d->item);

	if (d->transform)
		obs_sceneitem_set_info2(d->item, d->transform);
	if (d->crop)
		obs_sceneitem_set_crop(d->item, d->crop);
	obs_sceneitem_set_visible(d->item, d->enabled);
}

static void AddItemTask(void *param)
{
	auto *d = static_cast<AddItemData *>(param);
	obs_scene_atomic_update(d->scene, AddItemHelper, d);
}

obs_sceneitem_t *Utils::Obs::GraphicsTask::AddSceneItem(obs_scene_t *scene, obs_source_t *source, bool enabled,
							const obs_transform_info *transform,
							const obs_sceneitem_crop *crop)
{
	if (!(scene && source))
		return nullptr;

	AddItemData d = {scene, source, enabled, transform, crop, nullptr};
	RunGraphicsTask(AddItemTask, &d);
	return d.item;
}
