-- Neovim loads plugin/ at startup; the module lives under lua/.
vim.api.nvim_create_user_command("MyPlug", function()
  require("myplug").setup()
end, {})
local myplug = require("myplug")
myplug.setup()
