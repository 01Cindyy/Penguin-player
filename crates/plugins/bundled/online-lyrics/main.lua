-- Finds lyrics on LRCLIB (https://lrclib.net), a free and open lyrics database, for
-- songs that have none of their own, or for songs whose own lyrics the user would
-- rather replace. Only a song's title and artist are sent; its album and length stay
-- here and help pick the right match.

local search = require("search")

-- Songs without lyrics on LRCLIB are remembered, and searched for again after a
-- month, as lyrics keep being added.
local SEARCH_AGAIN_AFTER = 30 * 24 * 60 * 60
local MAX_REMEMBERED = 2000

-- The songs' own lyrics each "prefer" setting replaces.
local REPLACE = { never = "none", untimed = "untimed", always = "all" }

local function remembered()
  local missing = mp.storage.get("missing") or {}
  local now = os.time()
  local count = 0
  for key, when in pairs(missing) do
    if now - when > SEARCH_AGAIN_AFTER then
      missing[key] = nil
    else
      count = count + 1
    end
  end
  return missing, count
end

local function remember_missing(key)
  local missing, count = remembered()
  if count >= MAX_REMEMBERED then
    missing = {}
  end
  missing[key] = os.time()
  mp.storage.set("missing", missing)
end

local function find(track, done)
  -- A title alone matches too many songs.
  if track.artist == "" then
    done(nil)
    return
  end
  -- Untimed lyrics never replace timed ones, and replace untimed ones only when
  -- online lyrics are always preferred.
  local own = track.lyrics
  local plain = mp.settings.get("plain")
    and (own == nil or (own == "untimed" and mp.settings.get("prefer") == "always"))
  -- A song without timed lyrics may still have untimed ones.
  local key = search.key(track)
  local timed_key = key .. "\ntimed"
  local missing = remembered()
  if missing[key] or (not plain and missing[timed_key]) then
    done(nil)
    return
  end
  search.find(track, plain, function(lyrics, unreachable)
    if not lyrics and not unreachable then
      remember_missing(plain and key or timed_key)
    end
    done(lyrics)
  end)
end

local function provide()
  mp.lyrics.provide(find, { replace = REPLACE[mp.settings.get("prefer")] })
end

provide()
mp.on("setting_changed", function(key)
  if key == "prefer" then
    provide()
  end
end)
