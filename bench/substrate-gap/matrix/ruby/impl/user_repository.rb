require_relative "base_repository"

class UserRepository < BaseRepository
  def find(id)
    { id: id }
  end
end
