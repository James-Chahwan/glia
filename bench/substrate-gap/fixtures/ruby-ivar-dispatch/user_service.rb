class UserService
  def initialize
    @repo = UserRepo.new
  end

  def get(id)
    @repo.find(id)
  end

  def audit(x)
    @log ||= AuditLog.new
    @log.write(x)
  end
end
