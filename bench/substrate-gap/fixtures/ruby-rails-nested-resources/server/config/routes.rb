# HF.5: chatwoot's public routes.rb shape. A `resources` body nests its
# children under `/<r>/:<singular>_id`, `member` / `collection` blocks re-root
# their verbs at `/<r>/:id` / `/<r>`, and a plural `resources` answers at its
# `/:id` member path unless `only:` / `except:` leave no member action.
Rails.application.routes.draw do
  namespace :api do
    namespace :v1 do
      resources :accounts, only: [:show] do
        scope module: :accounts do
          resources :notifications, only: [:index, :destroy] do
            member do
              post :snooze
            end
            collection do
              post :read_all
            end
          end
          resources :labels, only: [:index]
          resources :conversations do
            resources :messages, only: [:index, :create]
          end
          namespace :captain do
            resources :assistants
          end
        end
      end
    end
  end
end
