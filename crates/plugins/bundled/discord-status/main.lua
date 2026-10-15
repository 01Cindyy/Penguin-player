-- Shows the playing song on the user's Discord profile, as "Listening to Penguin
-- Player". The picture is the app's icon, uploaded to its Discord application as
-- "penguin": Discord can't show covers from this computer.

local function setting(key)
  return mp.settings.get(key)
end

local function show()
  local track = mp.player.current_track()
  local state = mp.player.state()
  local paused = state == "paused"
  if not track or state == "stopped" or (paused and not setting("paused")) then
    mp.discord.clear()
    return
  end
  local activity = { type = "listening", large_image = "penguin" }
  if setting("title") then
    activity.details = track.title
  end
  if setting("artist") and track.artist ~= "" then
    activity.state = track.artist
  end
  if setting("album") and track.album ~= "" then
    activity.large_text = track.album
  end
  if paused then
    activity.state = activity.state and (activity.state .. " · Paused") or "Paused"
  elseif setting("progress") and track.duration_ms > 0 then
    activity.elapsed_ms = mp.player.position_ms()
    activity.duration_ms = track.duration_ms
  end
  local status = setting("status")
  if status == "artist" and activity.state and not paused then
    activity.status_display = "state"
  elseif status == "title" and activity.details then
    activity.status_display = "details"
  end
  mp.discord.set_activity(activity)
end

for _, event in ipairs({ "track_changed", "state_changed", "seeked", "setting_changed" }) do
  mp.on(event, show)
end
mp.on("shutdown", function()
  mp.discord.clear()
end)
