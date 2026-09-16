Rails.application.routes.draw do
  namespace :api do
    scope :v1 do
      get '/users/:id', to: 'users#show'
      resources :orders
    end
  end

  get '/health', to: 'health#index'
end
