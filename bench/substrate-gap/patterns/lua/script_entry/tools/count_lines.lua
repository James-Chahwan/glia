#!/usr/bin/env lua
-- A shell script: run as `count_lines.lua <file>`.
local path = arg[1]
local n = 0
for _ in io.lines(path) do
  n = n + 1
end
print(n)
