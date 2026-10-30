local function connect()
  local url = os.getenv("DATABASE_URL")
  return url
end

return { connect = connect }
