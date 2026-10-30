-- LOVE calls these by name: love.load once, then love.update / love.draw per frame.
local player = { x = 0 }

local function step(dt)
  player.x = player.x + dt
end

function love.load()
  player.x = 10
end

function love.update(dt)
  step(dt)
end

function love.draw()
  love.graphics.print("x=" .. player.x, 10, 10)
end
