(ns app.routes
  (:require [compojure.core :refer [defroutes GET POST]]
            [ring.util.response :refer [response status]]))

(defn get-user [id]
  (response {:id id}))

(defn create-user [request]
  (status (response {:ok true}) 201))

(defroutes app-routes
  (GET "/users/:id" [id] (get-user id))
  (POST "/users" request (create-user request)))
