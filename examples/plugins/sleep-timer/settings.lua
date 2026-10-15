-- Plugin settings kept in mp.storage, so they survive restarts.

local settings = {}

-- The stored number for `key`. A missing or invalid value is replaced by `default`
-- and saved, so the storage file always shows every setting with a sensible value.
function settings.number(key, default)
  local value = mp.storage.get(key)
  if type(value) ~= "number" or value < 0 then
    value = default
    mp.storage.set(key, value)
  end
  return value
end

return settings
