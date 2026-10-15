-- Searching LRCLIB and picking the best match for a song.

local search = {}

local API = "https://lrclib.net/api/search"
-- How far a match's length may be from the song's, in seconds.
local LENGTH_TOLERANCE = 3

-- What a song without lyrics is remembered by.
function search.key(track)
  return string.lower(track.artist .. "\n" .. track.title)
end

-- "Song (Remastered 2011)" or "Song - Live", without the addition; nil if there is
-- none.
local function simplified(title)
  local plain = title:gsub("%s*[%(%[].-[%)%]]", ""):gsub("%s+%-%s+.*$", "")
  plain = plain:match("^%s*(.-)%s*$")
  if plain ~= "" and plain ~= title then
    return plain
  end
end

local function text(value)
  if type(value) == "string" and value:match("%S") then
    return value
  end
end

-- How well a search result suits the song, and its lyrics; nil if it doesn't.
local function rate(result, track, plain)
  if type(result) ~= "table" or result.instrumental then
    return nil
  end
  local lyrics = text(result.syncedLyrics)
  local points = 100
  if not lyrics then
    lyrics = plain and text(result.plainLyrics)
    points = 0
  end
  if not lyrics then
    return nil
  end
  local length = track.duration_ms / 1000
  if length > 0 and type(result.duration) == "number" then
    local off = math.abs(result.duration - length)
    if off > LENGTH_TOLERANCE then
      return nil
    end
    points = points + (LENGTH_TOLERANCE - off) * 10
  end
  local album = text(result.albumName)
  if album and track.album ~= "" and album:lower() == track.album:lower() then
    points = points + 20
  end
  return points, lyrics
end

local function best(results, track, plain)
  local best_points, best_lyrics
  for _, result in ipairs(results) do
    local points, lyrics = rate(result, track, plain)
    if points and (not best_points or points > best_points) then
      best_points, best_lyrics = points, lyrics
    end
  end
  return best_lyrics
end

-- Calls found(lyrics) with the best lyrics for the track, found(nil) if LRCLIB has
-- none, or found(nil, true) if it couldn't be asked. Plain lyrics count only if
-- `plain` is true.
function search.find(track, plain, found)
  local function ask(title, otherwise)
    local options = { query = { track_name = title, artist_name = track.artist } }
    local started, problem = pcall(mp.http.get, API, options, function(response, err)
      if not response or not response.ok then
        mp.log.info("LRCLIB search failed:", err or ("status " .. response.status))
        found(nil, true)
        return
      end
      local decoded, results = pcall(mp.json.decode, response.body)
      local lyrics = decoded and type(results) == "table" and best(results, track, plain)
      if lyrics then
        found(lyrics)
      elseif otherwise then
        otherwise()
      else
        found(nil)
      end
    end)
    if not started then
      mp.log.warn("could not search LRCLIB:", problem)
      found(nil, true)
    end
  end
  local shorter = simplified(track.title)
  ask(track.title, shorter and function() ask(shorter) end)
end

return search
