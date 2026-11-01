-- Neovim: an autocmd binds a host event to a callback.
local function on_save(ev)
  print("saved " .. ev.file)
end
vim.api.nvim_create_autocmd("BufWritePost", { pattern = "*.lua", callback = on_save })

-- World of Warcraft: a frame registers for an event, OnEvent receives it.
local frame = CreateFrame("Frame")
frame:RegisterEvent("PLAYER_LOGIN")
local function on_event(self, event)
  print(event)
end
frame:SetScript("OnEvent", on_event)

-- AwesomeWM: a class signal, in a file that requires awful.
local awful = require("awful")
local function on_manage(c) awful.placement.no_offscreen(c) end
client.connect_signal("manage", on_manage)
