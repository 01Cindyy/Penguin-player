-- Searching LRCLIB and picking the best match for a song.

local search = {}

local API = "https://lrclib.net/api/search"
-- How far a match's length may be from the song's, in seconds.
local LENGTH_TOLERANCE = 3
-- Punctuation outside ASCII that names are written with either way.
local TYPOGRAPHY = { "‘", "’", "“", "”", "–", "—", "…" }

-- What a song without lyrics is remembered by.
function search.key(track)
  return string.lower(track.artist .. "\n" .. track.album .. "\n" .. track.title)
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

-- A title or album as it's compared: without additions, case, punctuation or spaces,
-- so "Nevermind (Remastered)" and "nevermind" are the same album.
local function comparable(name)
  local plain = (simplified(name) or name):lower()
  for _, mark in ipairs(TYPOGRAPHY) do
    plain = plain:gsub(mark, "")
  end
  plain = plain:gsub("[%p%s]", "")
  if plain == "" then
    return name:lower()
  end
  return plain
end

local function text(value)
  if type(value) == "string" and value:match("%S") then
    return value
  end
end

local function same(value, name)
  value = text(value)
  return value ~= nil and name ~= "" and comparable(value) == comparable(name)
end

-- How well a search result suits the song, and its lyrics; nil if it doesn't. A
-- result found by album must have the song's title and album, as only those two
-- vouch for it.
local function rate(result, track, plain, by_album)
  if type(result) ~= "table" or result.instrumental then
    return nil
  end
  local same_album = same(result.albumName, track.album)
  if by_album and not (same_album and same(result.trackName, track.title)) then
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
  if same_album then
    points = points + 20
  end
  return points, lyrics
end

local function best(results, track, plain, by_album)
  local best_points, best_lyrics
  for _, result in ipairs(results) do
    local points, lyrics = rate(result, track, plain, by_album)
    if points and (not best_points or points > best_points) then
      best_points, best_lyrics = points, lyrics
    end
  end
  return best_lyrics
end

-- The searches to try, in order: by title and artist, then by title and album, so a
-- song whose artist or album LRCLIB names a little differently is still found.
local function searches(track)
  local list = {}
  local shorter = simplified(track.title)
  if track.artist ~= "" then
    list[#list + 1] = { query = { track_name = track.title, artist_name = track.artist } }
    if shorter then
      list[#list + 1] = { query = { track_name = shorter, artist_name = track.artist } }
    end
  end
  if track.album ~= "" then
    -- Without their additions, the title and album also find other editions.
    list[#list + 1] = {
      query = { track_name = shorter or track.title,
                album_name = simplified(track.album) or track.album },
      by_album = true,
    }
  end
  return list
end

-- Calls found(lyrics) with the best lyrics for the track, found(nil) if LRCLIB has
-- none, or found(nil, true) if it couldn't be asked. Plain lyrics count only if
-- `plain` is true.
function search.find(track, plain, found)
  local list = searches(track)
  local function ask(index)
    local current = list[index]
    if not current then
      found(nil)
      return
    end
    local options = { query = current.query }
    local started, problem = pcall(mp.http.get, API, options, function(response, err)
      if not response or not response.ok then
        mp.log.info("LRCLIB search failed:", err or ("status " .. response.status))
        found(nil, true)
        return
      end
      local decoded, results = pcall(mp.json.decode, response.body)
      local lyrics = decoded and type(results) == "table"
        and best(results, track, plain, current.by_album)
      if lyrics then
        found(lyrics)
      else
        ask(index + 1)
      end
    end)
    if not started then
      mp.log.warn("could not search LRCLIB:", problem)
      found(nil, true)
    end
  end
  ask(1)
end

return search
