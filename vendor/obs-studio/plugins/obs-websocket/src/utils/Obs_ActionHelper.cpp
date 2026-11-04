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

#include "Obs.h"
#include "Obs_GraphicsTask.h"
#include "plugin-macros.generated.h"

obs_sceneitem_t *Utils::Obs::ActionHelper::CreateSceneItem(obs_source_t *source, obs_scene_t *scene, bool sceneItemEnabled,
							   obs_transform_info *sceneItemTransform,
							   obs_sceneitem_crop *sceneItemCrop)
{
	// Sanity check for valid scene
	if (!(source && scene))
		return nullptr;

	// camera-box#1387: the scene update runs on the graphics thread, not on this request thread
	return Utils::Obs::GraphicsTask::AddSceneItem(scene, source, sceneItemEnabled, sceneItemTransform, sceneItemCrop);
}

obs_sceneitem_t *Utils::Obs::ActionHelper::CreateInput(std::string inputName, std::string inputKind, obs_data_t *inputSettings,
						       obs_scene_t *scene, bool sceneItemEnabled)
{
	// Create the input
	OBSSourceAutoRelease input = obs_source_create(inputKind.c_str(), inputName.c_str(), inputSettings, nullptr);

	// Check that everything was created properly
	if (!input)
		return nullptr;

	// Apparently not all default input properties actually get applied on creation (smh)
	uint32_t flags = obs_source_get_output_flags(input);
	if ((flags & OBS_SOURCE_MONITOR_BY_DEFAULT) != 0)
		obs_source_set_monitoring_type(input, OBS_MONITORING_TYPE_MONITOR_ONLY);

	// Create a scene item for the input
	obs_sceneitem_t *ret = CreateSceneItem(input, scene, sceneItemEnabled);

	// If creation failed, remove the input
	if (!ret)
		obs_source_remove(input);

	return ret;
}

obs_source_t *Utils::Obs::ActionHelper::CreateSourceFilter(obs_source_t *source, std::string filterName, std::string filterKind,
							   obs_data_t *filterSettings)
{
	obs_source_t *filter = obs_source_create_private(filterKind.c_str(), filterName.c_str(), filterSettings);

	if (!filter)
		return nullptr;

	obs_source_filter_add(source, filter);

	return filter;
}

void Utils::Obs::ActionHelper::SetSourceFilterIndex(obs_source_t *source, obs_source_t *filter, size_t index)
{
	size_t currentIndex = Utils::Obs::NumberHelper::GetSourceFilterIndex(source, filter);
	obs_order_movement direction = index > currentIndex ? OBS_ORDER_MOVE_DOWN : OBS_ORDER_MOVE_UP;

	while (currentIndex != index) {
		obs_source_filter_set_order(source, filter, direction);

		if (direction == OBS_ORDER_MOVE_DOWN)
			currentIndex++;
		else
			currentIndex--;
	}
}
