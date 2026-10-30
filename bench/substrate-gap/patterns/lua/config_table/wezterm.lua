-- WezTerm reads the table this file returns as its configuration.
local wezterm = require("wezterm")
local config = wezterm.config_builder()

config.font_size = 12
config.color_scheme = "Gruvbox Dark"

return config
