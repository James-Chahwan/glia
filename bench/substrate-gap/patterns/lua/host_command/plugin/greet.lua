-- A Neovim plugin registers user commands with its host.
local function greet(opts)
  print("hi " .. opts.args)
end

vim.api.nvim_create_user_command("Greet", greet, { nargs = 1 })
vim.api.nvim_create_user_command("GreetAll", function()
  greet({ args = "all" })
end, {})
