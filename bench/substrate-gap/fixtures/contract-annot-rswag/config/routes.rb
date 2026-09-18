Rails.application.routes.draw do
  get '/users/:id', to: 'users#show'
end
