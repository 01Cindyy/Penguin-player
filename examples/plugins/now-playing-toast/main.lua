-- Now Playing Toast: shows a notification whenever a new track starts.

-- "24-bit / 96 kHz FLAC", "320 kbps MP3", or just "FLAC" when nothing else is known.
local function quality(track)
  local format = string.upper(track.format)
  if track.bit_depth and track.sample_rate then
    return string.format("%d-bit / %g kHz %s", track.bit_depth, track.sample_rate / 1000, format)
  elseif track.bitrate_kbps then
    return string.format("%d kbps %s", track.bitrate_kbps, format)
  end
  return format
end

-- "Artist — Album · quality", leaving out whatever is unknown.
local function details(track)
  local parts = {}
  if track.artist ~= "" then
    table.insert(parts, track.artist)
  end
  if track.album ~= "" then
    table.insert(parts, track.album)
  end
  local text = table.concat(parts, " — ")
  if text == "" then
    return quality(track)
  end
  return text .. " · " .. quality(track)
end

mp.on("track_changed", function(track)
  if track == nil then
    return
  end
  mp.notify(track.title, details(track))
end)

mp.log.info("ready")
