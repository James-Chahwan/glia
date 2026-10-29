local helper = require("helper")
local tool_view = require("maki.tool_view")

maki.api.register_tool({
  name = "greet",
  desc = "say hi",
  execute = function(args)
    return helper.greet(args[1])
  end,
  restore = function() end,
})
