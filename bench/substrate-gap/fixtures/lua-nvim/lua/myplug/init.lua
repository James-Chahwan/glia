local config = require("myplug.config")
local util = require("myplug.util")

local M = {}

local function apply(opts)
  util.merge(config.defaults(), opts)
end

function M.setup(opts)
  apply(opts or {})
end

return M
