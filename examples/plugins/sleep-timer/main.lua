-- Sleep Timer: once music starts playing, counts down and then fades out and pauses.
-- Pausing or stopping cancels the countdown; playing again starts a fresh one.
--
-- Settings live in this plugin's storage file (plugin-data/sleep-timer.json), which is
-- created with the defaults below the first time the plugin runs. Edit it and reload
-- plugins to change them.

local settings = require("settings")

local minutes = settings.number("minutes", 30)
local fade_seconds = settings.number("fade_seconds", 10)
local FADE_STEPS = 10

local countdown = nil -- timer id while counting down
local fade = nil -- timer id while fading out
local volume_before_fade = nil

local function stop_fading()
  if fade then
    mp.timer.cancel(fade)
    fade = nil
  end
  if volume_before_fade then
    mp.player.set_volume(volume_before_fade)
    volume_before_fade = nil
  end
end

local function cancel()
  if countdown then
    mp.timer.cancel(countdown)
    countdown = nil
  end
  stop_fading()
end

local function fall_asleep()
  mp.player.pause()
  stop_fading()
  mp.notify("Sleep timer", string.format("Paused after %g minutes. Sleep well!", minutes))
end

-- Lowers the volume in equal steps, then pauses and puts the volume back.
local function start_fading()
  countdown = nil
  if fade_seconds <= 0 then
    fall_asleep()
    return
  end
  volume_before_fade = mp.player.volume()
  local step = 0
  fade = mp.timer.every(fade_seconds * 1000 / FADE_STEPS, function()
    step = step + 1
    if step >= FADE_STEPS then
      fall_asleep()
    else
      mp.player.set_volume(volume_before_fade * (1 - step / FADE_STEPS))
    end
  end)
end

mp.on("state_changed", function(state)
  if state == "playing" then
    if countdown == nil and fade == nil then
      countdown = mp.timer.after(minutes * 60 * 1000, start_fading)
      mp.log.info(string.format("pausing in %g minutes", minutes))
    end
  else
    cancel()
  end
end)

-- Leave the volume as the user had it if the plugin is switched off mid-fade.
mp.on("shutdown", stop_fading)
