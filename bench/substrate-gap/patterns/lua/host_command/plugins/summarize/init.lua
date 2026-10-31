-- A Maki plugin registers a tool with its host (glia PR #3's case).
local function shorten(text)
  return text:sub(1, 80)
end

maki.api.register_tool({
  name = "summarize",
  execute = function(args)
    return shorten(args.text)
  end,
  restore = function() end,
})
