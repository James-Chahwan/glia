require "pg"

def connect
  PG.connect(ENV["DATABASE_URL"])
end
