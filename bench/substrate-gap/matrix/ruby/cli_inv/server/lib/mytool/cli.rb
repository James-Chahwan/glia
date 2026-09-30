require "thor"

module Mytool
  class CLI < Thor
    desc "sync", "Sync records"
    def sync
      puts "syncing"
    end
  end
end
