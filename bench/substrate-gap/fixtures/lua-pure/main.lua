-- Plain Lua: requires beside the file, under lib/, and a directory module.
local helper = require("helper")
local mathx = require("mathx")
local strings = require("strings")

local App = {}

local function log(msg)
  print(msg)
end

function App.new()
  local self = setmetatable({}, { __index = App })
  return self
end

function App:run()
  log("run")
  self:step()
  local total = mathx.add(1, 2)
  return strings.upper(helper.greet(total))
end

function App:step()
  local function tick() log("tick") end
  tick()
end

App.stop = function()
  log("stop")
end

return App
