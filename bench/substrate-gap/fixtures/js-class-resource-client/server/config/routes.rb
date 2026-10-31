Rails.application.routes.draw do
  namespace :api do
    namespace :v1 do
      scope 'accounts/:account_id' do
        resources :notifications, only: [:index, :destroy]
        post 'notifications/:id/unread', to: 'notifications#unread'
      end
      resource :profile, only: [:show, :update]
    end
  end
end
