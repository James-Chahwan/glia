# Rails `resources` is METHOD-AGNOSTIC: one declaration mounts GET/POST/PATCH/
# PUT/DELETE at /posts. The parser therefore emits a single ROUTE whose
# ROUTE_METHOD cell reads "ANY" (ruby/src/lib.rs `emit_rails_route`).
Rails.application.routes.draw do
  resources :posts
end
