class UsersController < ApplicationController
  def active
    @users = LegacyUser.where(active: true)
  end
end
