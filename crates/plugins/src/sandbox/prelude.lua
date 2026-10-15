-- Runs in every plugin's Lua state before any plugin code, hardening the standard
-- functions that the time limit can't police by itself (see time_limit.rs):
--
-- * pcall, xpcall, load and the coroutine functions re-raise the time-limit error, so
--   a plugin can't catch it and carry on.
-- * After the time-limit error, Lua runs some code with hooks disabled, where nothing
--   could stop it: xpcall message handlers, the __close handlers of a coroutine stopped
--   by the time limit once it is closed, and the host's own message handler, which
--   calls the error's __tostring. Message handlers are skipped once time is up, such
--   coroutines are never closed, and plugins can't give strings (the time-limit error
--   is one) metamethods.
-- * The time limit is checked before every call, but a library function that loops in
--   C can't be interrupted, and with elements fetched through a C metamethod (such as
--   __index = rawlen) even a table that uses no memory could keep it busy for hours.
--   table.move, insert, remove and concat therefore work in slices, one call each, and
--   table.sort compares through Lua code.
--
-- It also makes load text-only and refuses __gc finalizers, which would run plugin
-- code at arbitrary moments, outside any time limit.
--
-- Everything used here is kept in locals first, so a plugin replacing globals can't
-- change how these functions behave.

local check_time, time_is_up, readable = ...

local error, rawget, select, type = error, rawget, select, type
local min, tointeger, ult, maxinteger = math.min, math.tointeger, math.ult, math.maxinteger

-- The original functions. Called as fields of this table, Lua's own error messages
-- name them the way plugins know them ("bad argument #1 to 'sort'").
local lua = {
  setmetatable = setmetatable,
  load = load,
  pcall = pcall,
  xpcall = xpcall,
  create = coroutine.create,
  status = coroutine.status,
  resume = coroutine.resume,
  close = coroutine.close,
  move = table.move,
  sort = table.sort,
  concat = table.concat,
  rep = string.rep,
}

-- Elements handled by one call into C; a slice takes well under a millisecond.
local SLICE = 4096

local function check_type(value, expected, position, name)
  if type(value) ~= expected then
    error("bad argument #" .. position .. " to '" .. name .. "' (" .. expected
      .. " expected, got " .. type(value) .. ")", 3)
  end
end

local function check_integer(value, position, name)
  local integer = tointeger(value)
  if integer == nil then
    local problem = type(value) == "number" and "number has no integer representation"
      or "number expected, got " .. type(value)
    error("bad argument #" .. position .. " to '" .. name .. "' (" .. problem .. ")", 3)
  end
  return integer
end

local function length(list)
  local integer = tointeger(#list)
  if integer == nil then
    error("object length is not an integer", 3)
  end
  return integer
end

function setmetatable(table, metatable)
  if type(metatable) == "table" and rawget(metatable, "__gc") ~= nil then
    error("__gc metamethods are not available to plugins", 2)
  end
  return lua.setmetatable(table, metatable)
end

local function after_load(...)
  -- A reader function that ran out of time makes load fail like a syntax error would.
  check_time()
  return ...
end

-- load(chunk, chunkname, mode, env) with the mode forced to text. `env` is passed on
-- only when given: an explicit nil environment means "no globals".
function load(chunk, chunkname, _, ...)
  return after_load(lua.load(chunk, chunkname, "t", ...))
end

-- The results of a protected call: the error becomes a readable string, unless the
-- plugin is out of time, which stops it instead.
local function checked(ok, ...)
  if ok then
    return true, ...
  end
  check_time()
  return false, readable((...))
end

function pcall(f, ...)
  return checked(lua.pcall(f, ...))
end

function xpcall(f, handler, ...)
  check_type(handler, "function", 2, "xpcall")
  local function message_handler(err)
    if time_is_up() then
      return err
    end
    return handler(readable(err))
  end
  return checked(lua.xpcall(f, message_handler, ...))
end

-- Coroutines whose last resume the time limit may have cut short. A coroutine stopped
-- by the time limit keeps hooks disabled, so closing it would run its pending __close
-- handlers out of the limit's reach: these are never closed. Once time is up no plugin
-- code runs to record the stop, so a coroutine is recorded before it is resumed and
-- only forgotten when the resume comes back in time.
local interrupted = lua.setmetatable({}, { __mode = "k" })

local function forget_if_in_time(co, ...)
  if not time_is_up() then
    interrupted[co] = nil
  end
  return ...
end

local function resume(co, ...)
  if type(co) ~= "thread" or lua.status(co) ~= "suspended" then
    return lua.resume(co, ...)
  end
  interrupted[co] = true
  return forget_if_in_time(co, lua.resume(co, ...))
end

function coroutine.resume(co, ...)
  return checked(resume(co, ...))
end

function coroutine.close(co)
  if interrupted[co] then
    return false, "the coroutine was stopped by the time limit"
  end
  return checked(lua.close(co))
end

-- What a function made by coroutine.wrap returns: the coroutine's results, or its
-- error, re-raised after closing the coroutine like Lua's own wrap does.
local function unwrapped(co, ok, ...)
  if ok then
    return ...
  end
  check_time()
  local err = ...
  if lua.status(co) == "dead" and not interrupted[co] then
    local closed, close_error = lua.close(co)
    if not closed then
      err = close_error
    end
  end
  error(readable(err), type(err) == "string" and 2 or 0)
end

function coroutine.wrap(f)
  check_type(f, "function", 1, "wrap")
  local co = lua.create(f)
  return function(...)
    return unwrapped(co, resume(co, ...))
  end
end

-- All strings share one metatable, so a __tostring added there would run whenever an
-- error message is described. Plugins get a copy of it instead.
local string_metatable = getmetatable("")
local visible_string_metatable = {}
for key, value in next, string_metatable do
  visible_string_metatable[key] = value
end
string_metatable.__metatable = visible_string_metatable

-- table.move(a1, f, e, t, a2). The arguments are checked here, in the same order as
-- Lua's own move, so errors point at the plugin's line rather than this file.
local function move(a1, f, e, t, a2)
  local first = check_integer(f, 2, "move")
  local last = check_integer(e, 3, "move")
  local to = check_integer(t, 4, "move")
  check_type(a1, "table", 1, "move")
  if a2 == nil then
    a2 = a1
  else
    check_type(a2, "table", 5, "move")
  end
  if last < first then
    return a2
  end
  if first <= 0 and last >= maxinteger + first then
    error("bad argument #3 to 'move' (too many elements to move)", 2)
  end
  local count = last - first + 1
  if to > maxinteger - count + 1 then
    error("bad argument #4 to 'move' (destination wrap around)", 2)
  end
  if count <= SLICE then
    return lua.move(a1, first, last, to, a2)
  end
  -- Copy in the same direction as Lua does, so overlapping ranges come out right.
  local forwards = to > last or to <= first or a2 ~= a1
  local last_slice = (count - 1) // SLICE
  for i = 0, last_slice do
    local offset = (forwards and i or last_slice - i) * SLICE
    local size = min(SLICE, count - offset)
    lua.move(a1, first + offset, first + offset + size - 1, to + offset, a2)
  end
  return a2
end

table.move = move

function table.insert(list, ...)
  check_type(list, "table", 1, "insert")
  local count = select("#", ...)
  local after_last = length(list) + 1
  if count == 1 then
    list[after_last] = ...
  elseif count == 2 then
    local position, value = ...
    position = check_integer(position, 2, "insert")
    if not ult(position - 1, after_last) then
      error("bad argument #2 to 'insert' (position out of bounds)", 2)
    end
    move(list, position, after_last - 1, position + 1)
    list[position] = value
  else
    error("wrong number of arguments to 'insert'", 2)
  end
end

function table.remove(list, position)
  check_type(list, "table", 1, "remove")
  local size = length(list)
  if position == nil then
    position = size
  else
    position = check_integer(position, 2, "remove")
    if position ~= size and not (ult(position - 1, size) or position - 1 == size) then
      error("bad argument #2 to 'remove' (position out of bounds)", 2)
    end
  end
  local value = list[position]
  if position < size then
    move(list, position + 1, size, position)
    position = size
  end
  list[position] = nil
  return value
end

-- table.concat(list, sep, i, j), joining long ranges slice by slice.
function table.concat(list, separator, first, last)
  check_type(list, "table", 1, "concat")
  first = first == nil and 1 or check_integer(first, 3, "concat")
  last = last == nil and length(list) or check_integer(last, 4, "concat")
  -- `ult` compares the distance without overflowing on extreme bounds.
  if last < first or ult(last - first, SLICE) then
    return lua.concat(list, separator, first, last)
  end
  local slices = {}
  local start = first
  repeat
    local stop = ult(last - start, SLICE) and last or start + SLICE - 1
    slices[#slices + 1] = lua.concat(list, separator, start, stop)
    start = stop + 1
  until stop == last
  return lua.concat(slices, separator)
end

local function less_than(a, b)
  return a < b
end

-- table.sort(list, comp). Lua's sort compares in C, so every comparison goes through
-- a Lua function that the time limit can stop, even when `comp` is a C function.
function table.sort(list, comparator)
  check_type(list, "table", 1, "sort")
  local compare = less_than
  if comparator ~= nil then
    check_type(comparator, "function", 2, "sort")
    compare = function(a, b)
      return comparator(a, b)
    end
  end
  lua.sort(list, compare)
end

function string.rep(text, count, separator)
  -- Lua copies an empty string `count` times, which never ends for a huge count.
  if text == "" and (separator == nil or separator == "") then
    check_integer(count, 2, "rep")
    return ""
  end
  return lua.rep(text, count, separator)
end
